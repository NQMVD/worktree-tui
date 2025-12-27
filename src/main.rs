//! Git Worktree TUI - A beautiful terminal interface for managing Git worktrees
//! Designed with Claude's visual aesthetic: warm tones, clean typography, intuitive interactions

use anyhow::{Context, Result};
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers, MouseButton,
        MouseEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Margin, Rect},
    style::{Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Cell, Clear, List, ListItem, ListState, Padding, Paragraph,
        Row, Scrollbar, ScrollbarOrientation, ScrollbarState, Table, TableState, Wrap,
    },
    Frame, Terminal,
};
use std::{
    io::{self, Stdout},
    path::PathBuf,
    process::Command,
    time::{Duration, Instant},
};
use unicode_width::UnicodeWidthStr;

// ============================================================================
// Claude Design System - Warm, approachable colors inspired by Claude's aesthetic
// ============================================================================

#[allow(dead_code)]
mod colors {
    use ratatui::style::Color;

    // Primary palette - Claude's signature warm tones
    pub const CLAUDE_ORANGE: Color = Color::Rgb(217, 119, 87);
    pub const CLAUDE_CREAM: Color = Color::Rgb(250, 245, 235);
    pub const CLAUDE_WARM_GRAY: Color = Color::Rgb(120, 113, 108);
    pub const CLAUDE_DARK: Color = Color::Rgb(41, 37, 36);
    pub const CLAUDE_DARKER: Color = Color::Rgb(28, 25, 23);

    // Semantic colors
    pub const SUCCESS: Color = Color::Rgb(134, 239, 172);
    pub const WARNING: Color = Color::Rgb(253, 224, 71);
    pub const ERROR: Color = Color::Rgb(248, 113, 113);
    pub const INFO: Color = Color::Rgb(147, 197, 253);
    pub const PURPLE: Color = Color::Rgb(196, 181, 253);

    // UI elements
    pub const BORDER_ACTIVE: Color = CLAUDE_ORANGE;
    pub const BORDER_INACTIVE: Color = Color::Rgb(68, 64, 60);
    pub const SELECTION_BG: Color = Color::Rgb(38, 34, 30);
}

// ============================================================================
// Data Models
// ============================================================================

#[derive(Debug, Clone)]
struct Worktree {
    path: PathBuf,
    branch: Option<String>,
    commit: String,
    commit_short: String,
    commit_message: String,
    commit_time: Option<i64>,
    is_main: bool,
    is_bare: bool,
    is_detached: bool,
    is_locked: bool,
    lock_reason: Option<String>,
    is_prunable: bool,
    status: WorktreeStatus,
    recent_commits: Vec<CommitInfo>,
}

#[derive(Debug, Clone)]
struct CommitInfo {
    hash: String,
    message: String,
    time_ago: String,
}

#[derive(Debug, Clone, Default)]
struct WorktreeStatus {
    modified: usize,
    staged: usize,
    untracked: usize,
    ahead: usize,
    behind: usize,
}

impl WorktreeStatus {
    fn is_clean(&self) -> bool {
        self.modified == 0 && self.staged == 0 && self.untracked == 0
    }

    fn summary(&self) -> String {
        if self.is_clean() && self.ahead == 0 && self.behind == 0 {
            return String::from("clean");
        }

        let mut parts = Vec::new();
        if self.staged > 0 {
            parts.push(format!("+{}", self.staged));
        }
        if self.modified > 0 {
            parts.push(format!("~{}", self.modified));
        }
        if self.untracked > 0 {
            parts.push(format!("?{}", self.untracked));
        }
        if self.ahead > 0 {
            parts.push(format!("{}", self.ahead));
        }
        if self.behind > 0 {
            parts.push(format!("{}", self.behind));
        }
        parts.join(" ")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppMode {
    Normal,
    Help,
    Create,
    Delete,
    Search,
    BranchSelect,
    MergeSelect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusedPane {
    WorktreeList,
    Details,
}

#[derive(Debug, Clone)]
struct StatusMessage {
    text: String,
    level: MessageLevel,
    timestamp: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
enum MessageLevel {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Debug, Clone)]
struct Branch {
    name: String,
    is_remote: bool,
    is_current: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortOrder {
    Name,
    Status,
    Recent,
}

impl SortOrder {
    fn next(&self) -> Self {
        match self {
            SortOrder::Name => SortOrder::Status,
            SortOrder::Status => SortOrder::Recent,
            SortOrder::Recent => SortOrder::Name,
        }
    }

    fn label(&self) -> &'static str {
        match self {
            SortOrder::Name => "name",
            SortOrder::Status => "status",
            SortOrder::Recent => "recent",
        }
    }
}

// ============================================================================
// Application State
// ============================================================================

struct App {
    // Core state
    worktrees: Vec<Worktree>,
    table_state: TableState,
    mode: AppMode,
    focused_pane: FocusedPane,
    should_quit: bool,

    // Repository info
    repo_root: PathBuf,
    repo_name: String,

    // UI state
    status_message: Option<StatusMessage>,
    sort_order: SortOrder,
    show_recent_commits: bool,

    // Create dialog
    create_input: String,
    create_cursor: usize,
    available_branches: Vec<Branch>,
    branch_list_state: ListState,
    create_from_branch: Option<String>,
    create_checkout_existing: bool,
    merge_source_idx: Option<usize>,

    // Delete dialog
    delete_confirm: bool,

    // Search
    search_query: String,
    search_cursor: usize,
    filtered_indices: Vec<usize>,

    // Cached data
    last_refresh: Instant,

    // Mouse support
    list_area: Option<Rect>,
}

impl App {
    fn new() -> Result<Self> {
        let repo_root = Self::find_git_root()?;
        let repo_name = repo_root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "repository".to_string());

        let mut app = Self {
            worktrees: Vec::new(),
            table_state: TableState::default(),
            mode: AppMode::Normal,
            focused_pane: FocusedPane::WorktreeList,
            should_quit: false,

            repo_root,
            repo_name,

            status_message: None,
            sort_order: SortOrder::Name,
            show_recent_commits: true,

            create_input: String::new(),
            create_cursor: 0,
            available_branches: Vec::new(),
            branch_list_state: ListState::default(),
            create_from_branch: None,
            create_checkout_existing: false,
            merge_source_idx: None,

            delete_confirm: false,

            search_query: String::new(),
            search_cursor: 0,
            filtered_indices: Vec::new(),

            last_refresh: Instant::now(),

            list_area: None,
        };

        app.refresh_worktrees()?;
        if !app.worktrees.is_empty() {
            app.table_state.select(Some(0));
        }

        Ok(app)
    }

    fn find_git_root() -> Result<PathBuf> {
        let output = Command::new("git")
            .args(["rev-parse", "--show-toplevel"])
            .output()
            .context("Failed to execute git command")?;

        if !output.status.success() {
            anyhow::bail!("Not in a git repository");
        }

        let path = String::from_utf8(output.stdout)
            .context("Invalid UTF-8 in git output")?
            .trim()
            .to_string();

        Ok(PathBuf::from(path))
    }

    fn refresh_worktrees(&mut self) -> Result<()> {
        let output = Command::new("git")
            .current_dir(&self.repo_root)
            .args(["worktree", "list", "--porcelain"])
            .output()
            .context("Failed to list worktrees")?;

        if !output.status.success() {
            anyhow::bail!("git worktree list failed");
        }

        let content = String::from_utf8(output.stdout)?;
        self.worktrees = Self::parse_worktree_list(&content, &self.repo_root)?;
        self.last_refresh = Instant::now();

        // Fetch additional status for each worktree
        for worktree in &mut self.worktrees {
            if !worktree.is_bare {
                worktree.status = Self::get_worktree_status(&worktree.path);
                let commit_info = Self::get_commit_info(&worktree.path);
                worktree.commit_message = commit_info.0;
                worktree.commit_time = commit_info.1;
                worktree.recent_commits = Self::get_recent_commits(&worktree.path, 5);
            }
        }

        // Apply sorting
        self.apply_sort();

        // Update filtered indices
        if !self.search_query.is_empty() {
            self.update_search_filter();
        } else {
            self.filtered_indices = (0..self.worktrees.len()).collect();
        }

        self.set_status("Refreshed worktree list", MessageLevel::Info);
        Ok(())
    }

    fn apply_sort(&mut self) {
        match self.sort_order {
            SortOrder::Name => {
                self.worktrees.sort_by(|a, b| {
                    if a.is_main {
                        return std::cmp::Ordering::Less;
                    }
                    if b.is_main {
                        return std::cmp::Ordering::Greater;
                    }
                    a.branch.cmp(&b.branch)
                });
            }
            SortOrder::Status => {
                self.worktrees.sort_by(|a, b| {
                    if a.is_main {
                        return std::cmp::Ordering::Less;
                    }
                    if b.is_main {
                        return std::cmp::Ordering::Greater;
                    }
                    let a_dirty = !a.status.is_clean();
                    let b_dirty = !b.status.is_clean();
                    b_dirty.cmp(&a_dirty).then_with(|| a.branch.cmp(&b.branch))
                });
            }
            SortOrder::Recent => {
                self.worktrees.sort_by(|a, b| {
                    if a.is_main {
                        return std::cmp::Ordering::Less;
                    }
                    if b.is_main {
                        return std::cmp::Ordering::Greater;
                    }
                    b.commit_time.cmp(&a.commit_time)
                });
            }
        }
    }

    fn parse_worktree_list(content: &str, repo_root: &PathBuf) -> Result<Vec<Worktree>> {
        let mut worktrees = Vec::new();
        let mut current: Option<Worktree> = None;

        for line in content.lines() {
            if line.starts_with("worktree ") {
                if let Some(wt) = current.take() {
                    worktrees.push(wt);
                }
                let path = PathBuf::from(line.strip_prefix("worktree ").unwrap());
                let is_main = path == *repo_root;
                current = Some(Worktree {
                    path,
                    branch: None,
                    commit: String::new(),
                    commit_short: String::new(),
                    commit_message: String::new(),
                    commit_time: None,
                    is_main,
                    is_bare: false,
                    is_detached: false,
                    is_locked: false,
                    lock_reason: None,
                    is_prunable: false,
                    status: WorktreeStatus::default(),
                    recent_commits: Vec::new(),
                });
            } else if let Some(ref mut wt) = current {
                if line.starts_with("HEAD ") {
                    wt.commit = line.strip_prefix("HEAD ").unwrap().to_string();
                    wt.commit_short = wt.commit.chars().take(7).collect();
                } else if line.starts_with("branch ") {
                    let branch = line.strip_prefix("branch ").unwrap();
                    wt.branch = Some(
                        branch
                            .strip_prefix("refs/heads/")
                            .unwrap_or(branch)
                            .to_string(),
                    );
                } else if line == "bare" {
                    wt.is_bare = true;
                } else if line == "detached" {
                    wt.is_detached = true;
                } else if line == "locked" {
                    wt.is_locked = true;
                } else if line.starts_with("locked ") {
                    wt.is_locked = true;
                    wt.lock_reason = Some(line.strip_prefix("locked ").unwrap().to_string());
                } else if line == "prunable" {
                    wt.is_prunable = true;
                }
            }
        }

        if let Some(wt) = current {
            worktrees.push(wt);
        }

        Ok(worktrees)
    }

    fn get_worktree_status(path: &PathBuf) -> WorktreeStatus {
        let mut status = WorktreeStatus::default();

        if let Ok(output) = Command::new("git")
            .current_dir(path)
            .args(["status", "--porcelain=v1"])
            .output()
        {
            if output.status.success() {
                let content = String::from_utf8_lossy(&output.stdout);
                for line in content.lines() {
                    if line.len() < 2 {
                        continue;
                    }
                    let index = line.chars().next().unwrap();
                    let worktree = line.chars().nth(1).unwrap();

                    if index != ' ' && index != '?' {
                        status.staged += 1;
                    }
                    if worktree == 'M' || worktree == 'D' {
                        status.modified += 1;
                    }
                    if index == '?' {
                        status.untracked += 1;
                    }
                }
            }
        }

        if let Ok(output) = Command::new("git")
            .current_dir(path)
            .args(["rev-list", "--left-right", "--count", "HEAD...@{upstream}"])
            .output()
        {
            if output.status.success() {
                let content = String::from_utf8_lossy(&output.stdout);
                let parts: Vec<&str> = content.trim().split('\t').collect();
                if parts.len() == 2 {
                    status.ahead = parts[0].parse().unwrap_or(0);
                    status.behind = parts[1].parse().unwrap_or(0);
                }
            }
        }

        status
    }

    fn get_commit_info(path: &PathBuf) -> (String, Option<i64>) {
        let output = Command::new("git")
            .current_dir(path)
            .args(["log", "-1", "--format=%s|%ct"])
            .output();

        if let Ok(output) = output {
            if output.status.success() {
                let content = String::from_utf8_lossy(&output.stdout);
                let parts: Vec<&str> = content.trim().split('|').collect();
                if parts.len() >= 2 {
                    let message = parts[0].chars().take(60).collect();
                    let timestamp = parts[1].parse().ok();
                    return (message, timestamp);
                }
            }
        }
        (String::new(), None)
    }

    fn get_recent_commits(path: &PathBuf, count: usize) -> Vec<CommitInfo> {
        let output = Command::new("git")
            .current_dir(path)
            .args(["log", &format!("-{}", count), "--format=%h|%s|%cr"])
            .output();

        if let Ok(output) = output {
            if output.status.success() {
                let content = String::from_utf8_lossy(&output.stdout);
                return content
                    .lines()
                    .filter_map(|line| {
                        let parts: Vec<&str> = line.split('|').collect();
                        if parts.len() >= 3 {
                            Some(CommitInfo {
                                hash: parts[0].to_string(),
                                message: parts[1].chars().take(50).collect(),
                                time_ago: parts[2].to_string(),
                            })
                        } else {
                            None
                        }
                    })
                    .collect();
            }
        }
        Vec::new()
    }

    fn refresh_branches(&mut self) -> Result<()> {
        let mut branches = Vec::new();

        let output = Command::new("git")
            .current_dir(&self.repo_root)
            .args(["branch", "--format=%(refname:short)|%(HEAD)"])
            .output()?;

        if output.status.success() {
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                let parts: Vec<&str> = line.split('|').collect();
                if parts.len() >= 2 {
                    branches.push(Branch {
                        name: parts[0].to_string(),
                        is_remote: false,
                        is_current: parts[1] == "*",
                    });
                }
            }
        }

        let output = Command::new("git")
            .current_dir(&self.repo_root)
            .args(["branch", "-r", "--format=%(refname:short)"])
            .output()?;

        if output.status.success() {
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                let name = line.trim();
                if !name.contains("HEAD") {
                    branches.push(Branch {
                        name: name.to_string(),
                        is_remote: true,
                        is_current: false,
                    });
                }
            }
        }

        self.available_branches = branches;
        Ok(())
    }

    fn selected_worktree(&self) -> Option<&Worktree> {
        self.table_state
            .selected()
            .and_then(|i| self.filtered_indices.get(i))
            .and_then(|&idx| self.worktrees.get(idx))
    }

    fn set_status(&mut self, text: &str, level: MessageLevel) {
        self.status_message = Some(StatusMessage {
            text: text.to_string(),
            level,
            timestamp: Instant::now(),
        });
    }

    fn clear_old_status(&mut self) {
        if let Some(ref msg) = self.status_message {
            if msg.timestamp.elapsed() > Duration::from_secs(5) {
                self.status_message = None;
            }
        }
    }

    fn move_selection(&mut self, delta: i32) {
        let len = self.filtered_indices.len();
        if len == 0 {
            return;
        }

        let current = self.table_state.selected().unwrap_or(0);
        let new = if delta > 0 {
            (current + delta as usize).min(len - 1)
        } else {
            current.saturating_sub((-delta) as usize)
        };
        self.table_state.select(Some(new));
    }

    fn select_first(&mut self) {
        if !self.filtered_indices.is_empty() {
            self.table_state.select(Some(0));
        }
    }

    fn select_last(&mut self) {
        if !self.filtered_indices.is_empty() {
            self.table_state
                .select(Some(self.filtered_indices.len() - 1));
        }
    }

    fn update_search_filter(&mut self) {
        let query = self.search_query.to_lowercase();
        self.filtered_indices = self
            .worktrees
            .iter()
            .enumerate()
            .filter(|(_, wt)| {
                wt.path.to_string_lossy().to_lowercase().contains(&query)
                    || wt
                        .branch
                        .as_ref()
                        .map(|b| b.to_lowercase().contains(&query))
                        .unwrap_or(false)
                    || wt.commit_message.to_lowercase().contains(&query)
            })
            .map(|(i, _)| i)
            .collect();

        if self.table_state.selected().unwrap_or(0) >= self.filtered_indices.len() {
            self.table_state
                .select(if self.filtered_indices.is_empty() {
                    None
                } else {
                    Some(0)
                });
        }
    }

    // ===== Actions =====

    fn get_worktrees_dir(&self) -> PathBuf {
        let parent = self.repo_root.parent().unwrap_or(&self.repo_root);
        parent.join(format!("{}-worktrees", self.repo_name))
    }

    fn create_worktree(&mut self) -> Result<()> {
        let name = self.create_input.trim();
        if name.is_empty() {
            self.set_status("Worktree name cannot be empty", MessageLevel::Error);
            return Ok(());
        }

        // When checking out existing branch, a branch must be selected
        if self.create_checkout_existing && self.create_from_branch.is_none() {
            self.set_status("Select a branch to checkout (Tab)", MessageLevel::Error);
            return Ok(());
        }

        // Create worktrees in PROJECT-worktrees/ directory
        let worktrees_dir = self.get_worktrees_dir();

        // Ensure the worktrees directory exists
        if !worktrees_dir.exists() {
            if let Err(e) = std::fs::create_dir_all(&worktrees_dir) {
                self.set_status(
                    &format!("Failed to create worktrees dir: {}", e),
                    MessageLevel::Error,
                );
                return Ok(());
            }
        }

        let worktree_path = worktrees_dir.join(name);

        let mut args = vec!["worktree", "add"];

        if self.create_checkout_existing {
            // Checkout existing branch: git worktree add <path> <existing-branch>
            args.push(worktree_path.to_str().unwrap());
            args.push(self.create_from_branch.as_ref().unwrap());
        } else {
            // Create new branch: git worktree add -b <new-branch-name> <path> [<base-branch>]
            args.push("-b");
            args.push(name);
            args.push(worktree_path.to_str().unwrap());
            if let Some(ref branch) = self.create_from_branch {
                args.push(branch);
            }
        }

        let output = Command::new("git")
            .current_dir(&self.repo_root)
            .args(&args)
            .output()?;

        if output.status.success() {
            self.set_status(
                &format!("Created worktree: {}", name),
                MessageLevel::Success,
            );
            self.refresh_worktrees()?;
        } else {
            let error = String::from_utf8_lossy(&output.stderr);
            self.set_status(&format!("Failed: {}", error.trim()), MessageLevel::Error);
        }

        self.mode = AppMode::Normal;
        self.create_input.clear();
        self.create_cursor = 0;
        self.create_from_branch = None;
        self.create_checkout_existing = false;
        Ok(())
    }

    fn delete_worktree(&mut self) -> Result<()> {
        if let Some(wt) = self.selected_worktree().cloned() {
            if wt.is_main {
                self.set_status("Cannot delete main worktree", MessageLevel::Error);
                return Ok(());
            }

            let path = wt.path.to_string_lossy().to_string();
            let force = !wt.status.is_clean();

            let mut args = vec!["worktree", "remove"];
            if force {
                args.push("--force");
            }
            args.push(&path);

            let output = Command::new("git")
                .current_dir(&self.repo_root)
                .args(&args)
                .output()?;

            if output.status.success() {
                self.set_status(
                    &format!("Deleted worktree: {}", wt.branch.unwrap_or(path)),
                    MessageLevel::Success,
                );
                self.refresh_worktrees()?;
            } else {
                let error = String::from_utf8_lossy(&output.stderr);
                self.set_status(&format!("Failed: {}", error.trim()), MessageLevel::Error);
            }
        }

        self.mode = AppMode::Normal;
        self.delete_confirm = false;
        Ok(())
    }

    fn copy_path_to_clipboard(&mut self) {
        if let Some(wt) = self.selected_worktree() {
            let path = wt.path.to_string_lossy().to_string();

            #[cfg(target_os = "macos")]
            let result = Command::new("pbcopy")
                .stdin(std::process::Stdio::piped())
                .spawn()
                .and_then(|mut child| {
                    use std::io::Write;
                    if let Some(ref mut stdin) = child.stdin {
                        stdin.write_all(path.as_bytes())?;
                    }
                    child.wait()
                });

            #[cfg(target_os = "linux")]
            let result = Command::new("xclip")
                .args(["-selection", "clipboard"])
                .stdin(std::process::Stdio::piped())
                .spawn()
                .and_then(|mut child| {
                    use std::io::Write;
                    if let Some(ref mut stdin) = child.stdin {
                        stdin.write_all(path.as_bytes())?;
                    }
                    child.wait()
                });

            #[cfg(target_os = "windows")]
            let result = Command::new("clip")
                .stdin(std::process::Stdio::piped())
                .spawn()
                .and_then(|mut child| {
                    use std::io::Write;
                    if let Some(ref mut stdin) = child.stdin {
                        stdin.write_all(path.as_bytes())?;
                    }
                    child.wait()
                });

            #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
            let result: Result<std::process::ExitStatus, std::io::Error> = Err(
                std::io::Error::new(std::io::ErrorKind::Other, "Clipboard not supported"),
            );

            match result {
                Ok(_) => self.set_status(&format!("Copied: {}", path), MessageLevel::Success),
                Err(_) => self.set_status("Failed to copy to clipboard", MessageLevel::Error),
            }
        }
    }

    fn open_in_file_manager(&mut self) {
        if let Some(wt) = self.selected_worktree() {
            let path = wt.path.to_string_lossy().to_string();

            #[cfg(target_os = "macos")]
            let result = Command::new("open").arg(&path).spawn();

            #[cfg(target_os = "linux")]
            let result = Command::new("xdg-open").arg(&path).spawn();

            #[cfg(target_os = "windows")]
            let result = Command::new("explorer").arg(&path).spawn();

            #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
            let result: Result<std::process::Child, std::io::Error> = Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "File manager not supported",
            ));

            match result {
                Ok(_) => self.set_status("Opened in file manager", MessageLevel::Success),
                Err(_) => self.set_status("Failed to open file manager", MessageLevel::Error),
            }
        }
    }

    fn toggle_lock(&mut self) -> Result<()> {
        if let Some(wt) = self.selected_worktree().cloned() {
            let path = wt.path.to_string_lossy().to_string();
            let action = if wt.is_locked { "unlock" } else { "lock" };

            let output = Command::new("git")
                .current_dir(&self.repo_root)
                .args(["worktree", action, &path])
                .output()?;

            if output.status.success() {
                self.set_status(
                    &format!(
                        "{} worktree: {}",
                        if wt.is_locked { "Unlocked" } else { "Locked" },
                        wt.branch.unwrap_or(path)
                    ),
                    MessageLevel::Success,
                );
                self.refresh_worktrees()?;
            } else {
                let error = String::from_utf8_lossy(&output.stderr);
                self.set_status(&format!("Failed: {}", error.trim()), MessageLevel::Error);
            }
        }
        Ok(())
    }

    fn fetch_all(&mut self) -> Result<()> {
        self.set_status("Fetching from remote...", MessageLevel::Info);

        let output = Command::new("git")
            .current_dir(&self.repo_root)
            .args(["fetch", "--all", "--prune"])
            .output()?;

        if output.status.success() {
            self.set_status("Fetched latest from remote", MessageLevel::Success);
            self.refresh_worktrees()?;
        } else {
            self.set_status("Fetch failed", MessageLevel::Error);
        }
        Ok(())
    }

    fn pull_current(&mut self) -> Result<()> {
        if let Some(wt) = self.selected_worktree().cloned() {
            self.set_status("Pulling...", MessageLevel::Info);

            let output = Command::new("git")
                .current_dir(&wt.path)
                .args(["pull"])
                .output()?;

            if output.status.success() {
                self.set_status(
                    &format!("Pulled {}", wt.branch.unwrap_or_else(|| "worktree".into())),
                    MessageLevel::Success,
                );
                self.refresh_worktrees()?;
            } else {
                let error = String::from_utf8_lossy(&output.stderr);
                self.set_status(
                    &format!("Pull failed: {}", error.trim()),
                    MessageLevel::Error,
                );
            }
        }
        Ok(())
    }

    fn push_current(&mut self) -> Result<()> {
        if let Some(wt) = self.selected_worktree().cloned() {
            self.set_status("Pushing...", MessageLevel::Info);

            let output = Command::new("git")
                .current_dir(&wt.path)
                .args(["push"])
                .output()?;

            if output.status.success() {
                self.set_status(
                    &format!("Pushed {}", wt.branch.unwrap_or_else(|| "worktree".into())),
                    MessageLevel::Success,
                );
                self.refresh_worktrees()?;
            } else {
                let error = String::from_utf8_lossy(&output.stderr);
                self.set_status(
                    &format!("Push failed: {}", error.trim()),
                    MessageLevel::Error,
                );
            }
        }
        Ok(())
    }

    fn prune_worktrees(&mut self) -> Result<()> {
        self.set_status("Pruning stale worktrees...", MessageLevel::Info);

        let output = Command::new("git")
            .current_dir(&self.repo_root)
            .args(["worktree", "prune"])
            .output()?;

        if output.status.success() {
            self.set_status("Pruned stale worktrees", MessageLevel::Success);
            self.refresh_worktrees()?;
        } else {
            self.set_status("Prune failed", MessageLevel::Error);
        }
        Ok(())
    }

    fn perform_merge(&mut self, source_idx: usize, target_branch: String) -> Result<()> {
        let source_wt = &self.worktrees[source_idx];
        let source_branch = match &source_wt.branch {
            Some(b) => b.clone(),
            None => return Ok(()), // Should be handled by caller
        };

        if source_branch == target_branch {
            self.set_status("Cannot merge branch into itself", MessageLevel::Error);
            return Ok(());
        }

        // Find a worktree where target_branch is checked out
        let target_wt_path = self
            .worktrees
            .iter()
            .find(|wt| wt.branch.as_ref() == Some(&target_branch))
            .map(|wt| wt.path.clone());

        let merge_path = match target_wt_path {
            Some(path) => path,
            None => {
                // If not found, we could potentially try to merge in the main repo
                // if the main repo can switch to that branch.
                // For now, let's just support merging into active worktrees.
                self.set_status(
                    &format!("Branch {} is not active in any worktree", target_branch),
                    MessageLevel::Error,
                );
                return Ok(());
            }
        };

        self.set_status(
            &format!("Merging {} into {}...", source_branch, target_branch),
            MessageLevel::Info,
        );

        let output = Command::new("git")
            .current_dir(&merge_path)
            .args(["merge", &source_branch, "--no-edit"])
            .output()?;

        if output.status.success() {
            self.set_status(
                &format!("Merged {} into {}", source_branch, target_branch),
                MessageLevel::Success,
            );
            self.refresh_worktrees()?;
        } else {
            let error = String::from_utf8_lossy(&output.stderr);
            if error.contains("CONFLICT") || error.contains("conflict") {
                self.set_status(
                    &format!("Conflict! Resolve in: {}", merge_path.display()),
                    MessageLevel::Warning,
                );
            } else {
                self.set_status(
                    &format!("Merge failed: {}", error.trim()),
                    MessageLevel::Error,
                );
            }
        }
        Ok(())
    }

    fn get_main_branch_name(&self) -> String {
        // Try to detect the main branch name
        let output = Command::new("git")
            .current_dir(&self.repo_root)
            .args(["symbolic-ref", "refs/remotes/origin/HEAD", "--short"])
            .output();

        if let Ok(output) = output {
            if output.status.success() {
                let branch = String::from_utf8_lossy(&output.stdout)
                    .trim()
                    .strip_prefix("origin/")
                    .unwrap_or("main")
                    .to_string();
                return branch;
            }
        }

        // Fallback: check if main or master exists
        let output = Command::new("git")
            .current_dir(&self.repo_root)
            .args(["rev-parse", "--verify", "main"])
            .output();

        if let Ok(output) = output {
            if output.status.success() {
                return "main".to_string();
            }
        }

        "master".to_string()
    }

    fn refresh_merge_branches(&mut self) {
        let mut branches = Vec::new();
        let mut seen = std::collections::HashSet::new();

        for wt in &self.worktrees {
            if let Some(ref name) = wt.branch {
                if seen.insert(name.clone()) {
                    branches.push(Branch {
                        name: name.clone(),
                        is_remote: false,
                        is_current: wt.is_main,
                    });
                }
            }
        }

        // Sort active branches: main/master first, then alphabetically
        branches.sort_by(|a, b| {
            let a_is_main = a.name == "main" || a.name == "master";
            let b_is_main = b.name == "main" || b.name == "master";
            if a_is_main && !b_is_main {
                std::cmp::Ordering::Less
            } else if !a_is_main && b_is_main {
                std::cmp::Ordering::Greater
            } else {
                a.name.cmp(&b.name)
            }
        });

        self.available_branches = branches;
    }

    fn cycle_sort(&mut self) {
        self.sort_order = self.sort_order.next();
        self.apply_sort();
        self.filtered_indices = (0..self.worktrees.len()).collect();
        if !self.search_query.is_empty() {
            self.update_search_filter();
        }
        self.set_status(
            &format!("Sorted by {}", self.sort_order.label()),
            MessageLevel::Info,
        );
    }
}

// ============================================================================
// Event Handling
// ============================================================================

fn handle_events(app: &mut App) -> Result<bool> {
    if event::poll(Duration::from_millis(100))? {
        match event::read()? {
            Event::Key(key) => match app.mode {
                AppMode::Normal => handle_normal_mode(app, key.code, key.modifiers)?,
                AppMode::Help => handle_help_mode(app, key.code)?,
                AppMode::Create => handle_create_mode(app, key.code, key.modifiers)?,
                AppMode::Delete => handle_delete_mode(app, key.code)?,
                AppMode::Search => handle_search_mode(app, key.code, key.modifiers)?,
                AppMode::BranchSelect => handle_branch_select_mode(app, key.code)?,
                AppMode::MergeSelect => handle_merge_select_mode(app, key.code)?,
            },
            Event::Mouse(mouse) => {
                handle_mouse_event(app, mouse)?;
            }
            _ => {}
        }
    }
    app.clear_old_status();
    Ok(app.should_quit)
}

fn handle_mouse_event(app: &mut App, mouse: crossterm::event::MouseEvent) -> Result<()> {
    if app.mode != AppMode::Normal {
        return Ok(());
    }

    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if let Some(area) = app.list_area {
                if mouse.column >= area.x
                    && mouse.column < area.x + area.width
                    && mouse.row >= area.y
                    && mouse.row < area.y + area.height
                {
                    let row_offset = mouse.row.saturating_sub(area.y + 3);
                    let clicked_index = row_offset as usize;

                    if clicked_index < app.filtered_indices.len() {
                        app.table_state.select(Some(clicked_index));
                    }
                }
            }
        }
        MouseEventKind::ScrollDown => app.move_selection(1),
        MouseEventKind::ScrollUp => app.move_selection(-1),
        _ => {}
    }
    Ok(())
}

fn handle_normal_mode(app: &mut App, key: KeyCode, modifiers: KeyModifiers) -> Result<()> {
    match key {
        // Quit
        KeyCode::Char('q') | KeyCode::Esc => app.should_quit = true,

        // Navigation
        KeyCode::Char('j') | KeyCode::Down => app.move_selection(1),
        KeyCode::Char('k') | KeyCode::Up => app.move_selection(-1),
        KeyCode::Char('g') => app.select_first(),
        KeyCode::Char('G') => app.select_last(),
        KeyCode::Char('d') if modifiers.contains(KeyModifiers::CONTROL) => app.move_selection(5),
        KeyCode::Char('u') if modifiers.contains(KeyModifiers::CONTROL) => app.move_selection(-5),
        KeyCode::Home => app.select_first(),
        KeyCode::End => app.select_last(),
        KeyCode::PageDown => app.move_selection(10),
        KeyCode::PageUp => app.move_selection(-10),

        // Quick jump 1-9
        KeyCode::Char(c) if c.is_ascii_digit() && c != '0' => {
            let idx = c.to_digit(10).unwrap() as usize - 1;
            if idx < app.filtered_indices.len() {
                app.table_state.select(Some(idx));
            }
        }

        // Pane focus
        KeyCode::Tab => {
            app.focused_pane = match app.focused_pane {
                FocusedPane::WorktreeList => FocusedPane::Details,
                FocusedPane::Details => FocusedPane::WorktreeList,
            };
        }

        // Actions
        KeyCode::Char('c') | KeyCode::Char('a') => {
            app.mode = AppMode::Create;
            app.create_input.clear();
            app.create_cursor = 0;
            app.create_from_branch = None;
            app.create_checkout_existing = false;
            let _ = app.refresh_branches();
        }
        KeyCode::Char('x') | KeyCode::Delete => {
            if app.selected_worktree().is_some() {
                app.mode = AppMode::Delete;
                app.delete_confirm = false;
            }
        }
        KeyCode::Enter | KeyCode::Char('o') => {
            if let Some(wt) = app.selected_worktree() {
                app.set_status(
                    &format!("Path: {}", wt.path.to_string_lossy()),
                    MessageLevel::Info,
                );
            }
        }

        // New features
        KeyCode::Char('y') => app.copy_path_to_clipboard(),
        KeyCode::Char('O') => app.open_in_file_manager(),
        KeyCode::Char('p') => {
            let _ = app.pull_current();
        }
        KeyCode::Char('P') => {
            let _ = app.push_current();
        }
        KeyCode::Char('s') => app.cycle_sort(),
        KeyCode::Char('t') => app.show_recent_commits = !app.show_recent_commits,
        KeyCode::Char('L') => {
            let _ = app.toggle_lock();
        }
        KeyCode::Char('r') | KeyCode::Char('R') => {
            let _ = app.refresh_worktrees();
        }
        KeyCode::Char('F') => {
            let _ = app.fetch_all();
        }
        KeyCode::Char('X') => {
            let _ = app.prune_worktrees();
        }
        KeyCode::Char('m') => {
            if let Some(wt) = app.selected_worktree() {
                if wt.is_main && wt.branch.as_deref() == Some(&app.get_main_branch_name()) {
                    // It's the main branch in the main worktree,
                    // we can allow merging from it if the user wants to merge into something else.
                }

                if wt.branch.is_none() {
                    app.set_status("Cannot merge detached HEAD", MessageLevel::Error);
                } else {
                    let idx = app.table_state.selected().unwrap();
                    app.merge_source_idx = Some(app.filtered_indices[idx]);
                    app.mode = AppMode::MergeSelect;
                    app.refresh_merge_branches();
                    app.branch_list_state.select(Some(0));
                }
            }
        }

        KeyCode::Char('/') => {
            app.mode = AppMode::Search;
            app.search_query.clear();
            app.search_cursor = 0;
        }
        KeyCode::Char('?') => app.mode = AppMode::Help,

        _ => {}
    }
    Ok(())
}

fn handle_help_mode(app: &mut App, key: KeyCode) -> Result<()> {
    match key {
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') | KeyCode::Enter => {
            app.mode = AppMode::Normal;
        }
        _ => {}
    }
    Ok(())
}

fn handle_create_mode(app: &mut App, key: KeyCode, modifiers: KeyModifiers) -> Result<()> {
    match key {
        KeyCode::Esc => {
            app.mode = AppMode::Normal;
            app.create_input.clear();
            app.create_checkout_existing = false;
        }
        KeyCode::Enter => app.create_worktree()?,
        KeyCode::Char('b') => {
            app.create_checkout_existing = !app.create_checkout_existing;
        }
        KeyCode::Tab => {
            app.mode = AppMode::BranchSelect;
            app.branch_list_state.select(Some(0));
        }
        KeyCode::Backspace => {
            if app.create_cursor > 0 {
                app.create_input.remove(app.create_cursor - 1);
                app.create_cursor -= 1;
            }
        }
        KeyCode::Left => app.create_cursor = app.create_cursor.saturating_sub(1),
        KeyCode::Right => app.create_cursor = (app.create_cursor + 1).min(app.create_input.len()),
        KeyCode::Char('u') if modifiers.contains(KeyModifiers::CONTROL) => {
            app.create_input.clear();
            app.create_cursor = 0;
        }
        KeyCode::Char(c) => {
            app.create_input.insert(app.create_cursor, c);
            app.create_cursor += 1;
        }
        _ => {}
    }
    Ok(())
}

fn handle_delete_mode(app: &mut App, key: KeyCode) -> Result<()> {
    match key {
        KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
            app.mode = AppMode::Normal;
            app.delete_confirm = false;
        }
        KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => app.delete_worktree()?,
        _ => {}
    }
    Ok(())
}

fn handle_search_mode(app: &mut App, key: KeyCode, modifiers: KeyModifiers) -> Result<()> {
    match key {
        KeyCode::Esc => {
            app.mode = AppMode::Normal;
            app.search_query.clear();
            app.filtered_indices = (0..app.worktrees.len()).collect();
        }
        KeyCode::Enter => app.mode = AppMode::Normal,
        KeyCode::Backspace => {
            if app.search_cursor > 0 {
                app.search_query.remove(app.search_cursor - 1);
                app.search_cursor -= 1;
                app.update_search_filter();
            }
        }
        KeyCode::Char('u') if modifiers.contains(KeyModifiers::CONTROL) => {
            app.search_query.clear();
            app.search_cursor = 0;
            app.update_search_filter();
        }
        KeyCode::Char(c) => {
            app.search_query.insert(app.search_cursor, c);
            app.search_cursor += 1;
            app.update_search_filter();
        }
        _ => {}
    }
    Ok(())
}

fn handle_branch_select_mode(app: &mut App, key: KeyCode) -> Result<()> {
    match key {
        KeyCode::Esc => app.mode = AppMode::Create,
        KeyCode::Enter => {
            if let Some(idx) = app.branch_list_state.selected() {
                if let Some(branch) = app.available_branches.get(idx) {
                    app.create_from_branch = Some(branch.name.clone());
                }
            }
            app.mode = AppMode::Create;
        }
        KeyCode::Char('j') | KeyCode::Down => {
            let len = app.available_branches.len();
            if len > 0 {
                let current = app.branch_list_state.selected().unwrap_or(0);
                app.branch_list_state.select(Some((current + 1) % len));
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            let len = app.available_branches.len();
            if len > 0 {
                let current = app.branch_list_state.selected().unwrap_or(0);
                app.branch_list_state.select(Some(if current == 0 {
                    len - 1
                } else {
                    current - 1
                }));
            }
        }
        _ => {}
    }
    Ok(())
}

fn handle_merge_select_mode(app: &mut App, key: KeyCode) -> Result<()> {
    match key {
        KeyCode::Esc => {
            app.mode = AppMode::Normal;
            app.merge_source_idx = None;
        }
        KeyCode::Enter => {
            let target_branch = if let Some(idx) = app.branch_list_state.selected() {
                app.available_branches.get(idx).map(|b| b.name.clone())
            } else {
                None
            };

            if let (Some(source_idx), Some(target)) = (app.merge_source_idx, target_branch) {
                app.perform_merge(source_idx, target)?;
            }
            app.mode = AppMode::Normal;
            app.merge_source_idx = None;
        }
        KeyCode::Char('j') | KeyCode::Down => {
            let len = app.available_branches.len();
            if len > 0 {
                let current = app.branch_list_state.selected().unwrap_or(0);
                app.branch_list_state.select(Some((current + 1) % len));
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            let len = app.available_branches.len();
            if len > 0 {
                let current = app.branch_list_state.selected().unwrap_or(0);
                app.branch_list_state.select(Some(if current == 0 {
                    len - 1
                } else {
                    current - 1
                }));
            }
        }
        _ => {}
    }
    Ok(())
}

// ============================================================================
// UI Rendering
// ============================================================================

fn ui(frame: &mut Frame, app: &mut App) {
    let size = frame.area();

    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(3),
        ])
        .split(size);

    render_header(frame, app, main_chunks[0]);
    render_content(frame, app, main_chunks[1]);
    render_status_bar(frame, app, main_chunks[2]);

    match app.mode {
        AppMode::Help => render_help_dialog(frame),
        AppMode::Create => render_create_dialog(frame, app),
        AppMode::Delete => render_delete_dialog(frame, app),
        AppMode::BranchSelect => {
            render_create_dialog(frame, app);
            render_branch_select_dialog(frame, app, "Select Base Branch");
        }
        AppMode::MergeSelect => {
            render_branch_select_dialog(frame, app, "Merge Into Branch");
        }
        AppMode::Search => render_search_bar(frame, app),
        _ => {}
    }
}

fn render_header(frame: &mut Frame, app: &App, area: Rect) {
    let title_block = Block::default()
        // .borders(Borders::BOTTOM)
        // .border_type(BorderType::LightDoubleDashed)
        // .border_style(Style::default().fg(colors::BORDER_INACTIVE))
        .padding(Padding::horizontal(1));

    let inner = title_block.inner(area);
    frame.render_widget(title_block, area);

    let header_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(inner);

    let logo = Line::from(vec![
        Span::styled("  ", Style::default().fg(colors::CLAUDE_ORANGE)),
        Span::styled("Worktree", Style::default().fg(colors::CLAUDE_CREAM)),
        Span::raw(" "),
        Span::styled(":: ", Style::default().fg(colors::CLAUDE_WARM_GRAY)),
        Span::styled(
            &app.repo_name,
            Style::default().fg(colors::CLAUDE_WARM_GRAY),
        ),
    ]);
    frame.render_widget(Paragraph::new(logo), header_layout[0]);

    let total = app.worktrees.len();
    let dirty = app
        .worktrees
        .iter()
        .filter(|w| !w.status.is_clean())
        .count();

    let mut stats_spans = vec![Span::styled(
        format!("{} worktrees", total),
        Style::default().fg(colors::CLAUDE_WARM_GRAY),
    )];
    if dirty > 0 {
        stats_spans.push(Span::styled(
            format!("  {} dirty", dirty),
            Style::default().fg(colors::WARNING),
        ));
    }
    stats_spans.extend([
        Span::raw("  "),
        Span::styled(
            format!("  {}", app.sort_order.label()),
            Style::default().fg(colors::CLAUDE_WARM_GRAY),
        ),
        Span::raw("  "),
        Span::styled("?", Style::default().fg(colors::CLAUDE_ORANGE)),
        Span::styled(" help", Style::default().fg(colors::CLAUDE_WARM_GRAY)),
    ]);

    let stats = Line::from(stats_spans).alignment(Alignment::Right);
    frame.render_widget(Paragraph::new(stats), header_layout[1]);
}

fn render_content(frame: &mut Frame, app: &mut App, area: Rect) {
    let content_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(area);

    render_worktree_list(frame, app, content_chunks[0]);
    render_details_panel(frame, app, content_chunks[1]);
}

fn render_worktree_list(frame: &mut Frame, app: &mut App, area: Rect) {
    app.list_area = Some(area);

    let is_focused = app.focused_pane == FocusedPane::WorktreeList;
    let border_color = if is_focused {
        colors::BORDER_ACTIVE
    } else {
        colors::BORDER_INACTIVE
    };

    let block = Block::default()
        .title(Line::from(vec![
            Span::raw(" "),
            Span::styled(
                "Worktrees",
                Style::default()
                    .fg(if is_focused {
                        colors::CLAUDE_ORANGE
                    } else {
                        colors::CLAUDE_CREAM
                    }),
            ),
            Span::raw(" "),
        ]))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color))
        .padding(Padding::horizontal(1));

    let header_cells = ["#", "", "Branch", "Status", "Commit"]
        .iter()
        .map(|h| Cell::from(*h).style(Style::default().fg(colors::CLAUDE_WARM_GRAY)));
    let header = Row::new(header_cells).height(1).bottom_margin(1);

    let rows: Vec<Row> = app
        .filtered_indices
        .iter()
        .enumerate()
        .map(|(display_idx, &idx)| {
            let wt = &app.worktrees[idx];

            let num = if display_idx < 9 {
                Span::styled(
                    format!("{}", display_idx + 1),
                    Style::default().fg(colors::CLAUDE_WARM_GRAY),
                )
            } else {
                Span::raw(" ")
            };

            let icon = if wt.is_main {
                Span::styled("", Style::default().fg(colors::CLAUDE_ORANGE))
            } else if wt.is_locked {
                Span::styled("", Style::default().fg(colors::WARNING))
            } else if wt.is_prunable {
                Span::styled("", Style::default().fg(colors::ERROR))
            } else {
                Span::styled("", Style::default().fg(colors::INFO))
            };

            let branch_name = wt.branch.as_deref().unwrap_or(if wt.is_detached {
                "(detached)"
            } else {
                "(bare)"
            });
            let branch_style = if wt.is_main {
                Style::default().fg(colors::CLAUDE_ORANGE)
            } else if wt.is_detached {
                Style::default().fg(colors::WARNING)
            } else {
                Style::default().fg(colors::CLAUDE_CREAM)
            };

            let status_style = if wt.status.is_clean() {
                Style::default().fg(colors::SUCCESS)
            } else {
                Style::default().fg(colors::WARNING)
            };

            Row::new(vec![
                Cell::from(num),
                Cell::from(icon),
                Cell::from(Span::styled(branch_name, branch_style)),
                Cell::from(Span::styled(wt.status.summary(), status_style)),
                Cell::from(Span::styled(
                    &wt.commit_short,
                    Style::default().fg(colors::CLAUDE_WARM_GRAY),
                )),
            ])
            .height(1)
        })
        .collect();

    let widths = [
        Constraint::Length(2),
        Constraint::Length(2),
        Constraint::Min(12),
        Constraint::Length(12),
        Constraint::Length(8),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .row_highlight_style(
            Style::default().bg(colors::SELECTION_BG), // .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(" ");

    frame.render_stateful_widget(table, area, &mut app.table_state);

    // Scrollbar
    if app.filtered_indices.len() > (area.height - 4) as usize {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some(""))
            .end_symbol(Some(""));
        let mut scrollbar_state = ScrollbarState::new(app.filtered_indices.len())
            .position(app.table_state.selected().unwrap_or(0));
        frame.render_stateful_widget(
            scrollbar,
            area.inner(Margin {
                vertical: 1,
                horizontal: 0,
            }),
            &mut scrollbar_state,
        );
    }
}

fn render_details_panel(frame: &mut Frame, app: &App, area: Rect) {
    let is_focused = app.focused_pane == FocusedPane::Details;
    let border_color = if is_focused {
        colors::BORDER_ACTIVE
    } else {
        colors::BORDER_INACTIVE
    };

    let block = Block::default()
        .title(Line::from(vec![
            Span::raw(" "),
            Span::styled(
                "Details",
                Style::default()
                    .fg(if is_focused {
                        colors::CLAUDE_ORANGE
                    } else {
                        colors::CLAUDE_CREAM
                    }),
            ),
            Span::raw(" "),
        ]))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color))
        .padding(Padding::new(2, 2, 1, 1));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if let Some(wt) = app.selected_worktree() {
        let mut lines = Vec::new();

        // --- Identity & Status ---
        let branch_name = wt.branch.as_deref().unwrap_or(if wt.is_detached { "(detached)" } else { "(bare)" });
        lines.push(Line::from(vec![
            Span::styled(branch_name, Style::default().fg(colors::CLAUDE_ORANGE)),
            Span::raw(" "),
            if wt.is_main {
                Span::styled("[MAIN]", Style::default().fg(colors::PURPLE))
            } else {
                Span::raw("")
            },
        ]));

        let mut status_spans = vec![Span::raw("  ")];
        if wt.status.is_clean() {
            status_spans.push(Span::styled("Clean", Style::default().fg(colors::SUCCESS)));
        } else {
            status_spans.push(Span::styled("Modified", Style::default().fg(colors::WARNING)));
            status_spans.push(Span::raw(" ("));
            let mut parts = Vec::new();
            if wt.status.staged > 0 { parts.push(Span::styled(format!("+{}", wt.status.staged), Style::default().fg(colors::SUCCESS))); }
            if wt.status.modified > 0 { parts.push(Span::styled(format!("~{}", wt.status.modified), Style::default().fg(colors::WARNING))); }
            if wt.status.untracked > 0 { parts.push(Span::styled(format!("?{}", wt.status.untracked), Style::default().fg(colors::CLAUDE_WARM_GRAY))); }
            
            for (i, part) in parts.into_iter().enumerate() {
                if i > 0 { status_spans.push(Span::raw(" ")); }
                status_spans.push(part);
            }
            status_spans.push(Span::raw(")"));
        }
        
        if wt.status.ahead > 0 || wt.status.behind > 0 {
            status_spans.push(Span::styled(" • ", Style::default().fg(colors::CLAUDE_WARM_GRAY)));
            if wt.status.ahead > 0 {
                status_spans.push(Span::styled(format!("↑{}", wt.status.ahead), Style::default().fg(colors::SUCCESS)));
                if wt.status.behind > 0 { status_spans.push(Span::raw(" ")); }
            }
            if wt.status.behind > 0 {
                status_spans.push(Span::styled(format!("↓{}", wt.status.behind), Style::default().fg(colors::ERROR)));
            }
        }
        lines.push(Line::from(status_spans));
        lines.push(Line::raw(""));

        // --- Location ---
        lines.push(Line::from(Span::styled("Location", Style::default().fg(colors::CLAUDE_WARM_GRAY))));
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(truncate_path(&wt.path, inner.width.saturating_sub(4) as usize), Style::default().fg(colors::CLAUDE_CREAM)),
        ]));
        lines.push(Line::raw(""));

        // --- Current Commit ---
        lines.push(Line::from(Span::styled("Current Commit", Style::default().fg(colors::CLAUDE_WARM_GRAY))));
        let time_ago = wt.recent_commits.first().map(|c| c.time_ago.clone()).unwrap_or_default();
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(&wt.commit_short, Style::default().fg(colors::INFO)),
            Span::styled(format!(" • {}", time_ago), Style::default().fg(colors::CLAUDE_WARM_GRAY).italic()),
        ]));
        
        if !wt.commit_message.is_empty() {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(&wt.commit_message, Style::default().fg(colors::CLAUDE_CREAM).italic()),
            ]));
        }
        lines.push(Line::raw(""));

        // --- Attributes ---
        if wt.is_locked || wt.is_prunable {
            lines.push(Line::from(Span::styled("Attributes", Style::default().fg(colors::CLAUDE_WARM_GRAY))));
            if wt.is_locked {
                lines.push(Line::from(vec![
                    Span::raw("  Locked: "),
                    Span::styled(wt.lock_reason.as_deref().unwrap_or("no reason provided"), Style::default().fg(colors::WARNING).italic()),
                ]));
            }
            if wt.is_prunable {
                lines.push(Line::from(vec![
                    Span::raw("  Prunable: "),
                    Span::styled("Worktree path is missing or invalid", Style::default().fg(colors::ERROR).italic()),
                ]));
            }
            lines.push(Line::raw(""));
        }

        // --- History ---
        if app.show_recent_commits && wt.recent_commits.len() > 1 {
            lines.push(Line::from(vec![
                Span::styled("Recent History", Style::default().fg(colors::CLAUDE_WARM_GRAY)),
                Span::styled(" (t to toggle)", Style::default().fg(colors::CLAUDE_WARM_GRAY).italic()),
            ]));
            
            for commit in wt.recent_commits.iter().skip(1).take(4) {
                let msg = truncate_str(&commit.message, inner.width.saturating_sub(16) as usize);
                lines.push(Line::from(vec![
                    Span::styled(format!("  {} ", commit.hash), Style::default().fg(colors::PURPLE)),
                    Span::styled(msg, Style::default().fg(colors::CLAUDE_WARM_GRAY)),
                ]));
            }
        }

        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
    } else {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "No worktree selected",
                Style::default().fg(colors::CLAUDE_WARM_GRAY).italic(),
            ))
            .alignment(Alignment::Center),
            inner,
        );
    }
}

fn render_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        // .borders(Borders::TOP)
        // .border_type(BorderType::LightDoubleDashed)
        // .border_style(Style::default().fg(colors::BORDER_INACTIVE))
        .padding(Padding::top(1));
    // .padding(Padding::horizontal(1));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(inner);

    let mode_hints = match app.mode {
        AppMode::Normal => vec![
            ("j/k", "nav"),
            ("1-9", "jump"),
            ("y", "copy"),
            ("p/P", "pull/push"),
            ("m", "merge"),
            ("s", "sort"),
            ("/", "search"),
        ],
        AppMode::Search => vec![("Enter", "confirm"), ("Esc", "cancel")],
        _ => vec![("Esc", "cancel")],
    };

    let hints: Vec<Span> = mode_hints
        .iter()
        .flat_map(|(key, action)| {
            vec![
                Span::styled(*key, Style::default().fg(colors::CLAUDE_ORANGE)),
                Span::styled(
                    format!(" {}  ", action),
                    Style::default().fg(colors::CLAUDE_WARM_GRAY),
                ),
            ]
        })
        .collect();

    frame.render_widget(
        Paragraph::new(Line::from(hints)),
        Rect::new(layout[0].x + 1, layout[0].y, layout[0].width, 1),
    );

    if let Some(ref msg) = app.status_message {
        let color = match msg.level {
            MessageLevel::Info => colors::INFO,
            MessageLevel::Success => colors::SUCCESS,
            MessageLevel::Warning => colors::WARNING,
            MessageLevel::Error => colors::ERROR,
        };
        frame.render_widget(
            Paragraph::new(Span::styled(&msg.text, Style::default().fg(color)))
                .alignment(Alignment::Right),
            Rect::new(layout[1].x, layout[1].y, layout[1].width - 1, 1),
        );
    }
}

fn render_help_dialog(frame: &mut Frame) {
    let area = centered_rect(65, 75, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(Line::from(vec![
            Span::raw(" "),
            Span::styled(
                "Keyboard Shortcuts",
                Style::default().fg(colors::CLAUDE_ORANGE).bold(),
            ),
            Span::raw(" "),
        ]))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(colors::CLAUDE_ORANGE))
        .style(Style::default().bg(colors::CLAUDE_DARKER))
        .padding(Padding::new(2, 2, 1, 1));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let help_text = vec![
        (
            "Navigation",
            vec![
                "j/k /        Move down/up",
                "g / G            Go to first/last",
                "1-9              Jump to item",
                "Ctrl+d/u         Page down/up",
                "Tab              Switch pane",
            ],
        ),
        (
            "Git Operations",
            vec![
                "c / a            Create worktree",
                "  b              Toggle new/existing branch",
                "x / Del          Delete worktree",
                "L                Toggle lock",
                "p                Pull (in worktree)",
                "P                Push (from worktree)",
                "F                Fetch all remotes",
                "r / R            Refresh list",
                "X                Prune stale",
                "m                Merge branch",
            ],
        ),
        (
            "Utilities",
            vec![
                "y                Copy path to clipboard",
                "O                Open in file manager",
                "s                Cycle sort order",
                "t                Toggle recent commits",
                "/                Search worktrees",
                "?                Toggle this help",
                "q / Esc          Quit",
            ],
        ),
    ];

    let mut y = 0;
    for (section, items) in help_text {
        frame.render_widget(
            Paragraph::new(Span::styled(
                section,
                Style::default().fg(colors::CLAUDE_CREAM).bold(),
            )),
            Rect::new(inner.x, inner.y + y, inner.width, 1),
        );
        y += 1;

        for item in items {
            frame.render_widget(
                Paragraph::new(Span::styled(
                    format!("  {}", item),
                    Style::default().fg(colors::CLAUDE_WARM_GRAY),
                )),
                Rect::new(inner.x, inner.y + y, inner.width, 1),
            );
            y += 1;
        }
        y += 1;
    }

    frame.render_widget(
        Paragraph::new(Span::styled(
            "Press Esc or ? to close",
            Style::default().fg(colors::CLAUDE_WARM_GRAY).italic(),
        ))
        .alignment(Alignment::Center),
        Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1),
    );
}

fn render_create_dialog(frame: &mut Frame, app: &App) {
    let area = centered_rect(50, 40, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(Line::from(vec![
            Span::raw(" "),
            Span::styled(
                "Create Worktree",
                Style::default().fg(colors::CLAUDE_ORANGE).bold(),
            ),
            Span::raw(" "),
        ]))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(colors::CLAUDE_ORANGE))
        .style(Style::default().bg(colors::CLAUDE_DARKER))
        .padding(Padding::new(2, 2, 1, 1));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Mode toggle indicator
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Mode: ", Style::default().fg(colors::CLAUDE_WARM_GRAY)),
            Span::styled(
                if app.create_checkout_existing {
                    "[Checkout Existing]"
                } else {
                    "[Create New Branch]"
                },
                Style::default()
                    .fg(if app.create_checkout_existing {
                        colors::INFO
                    } else {
                        colors::SUCCESS
                    })
                    .bold(),
            ),
            Span::raw(" "),
            Span::styled("(b to toggle)", Style::default().fg(colors::CLAUDE_WARM_GRAY).italic()),
        ])),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );

    let label = if app.create_checkout_existing {
        "Worktree directory:"
    } else {
        "Worktree name:"
    };

    let label_y = inner.y + 2;
    frame.render_widget(
        Paragraph::new(Span::styled(label, Style::default().fg(colors::CLAUDE_CREAM))),
        Rect::new(inner.x, label_y, inner.width, 1),
    );

    let input_area = Rect::new(inner.x, label_y + 2, inner.width, 3);
    let input_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(if app.mode == AppMode::Create {
            colors::CLAUDE_ORANGE
        } else {
            colors::BORDER_INACTIVE
        }));

    frame.render_widget(
        Paragraph::new(app.create_input.as_str())
            .block(input_block)
            .style(Style::default().fg(colors::CLAUDE_CREAM)),
        input_area,
    );

    if app.mode == AppMode::Create {
        frame.set_cursor_position((
            input_area.x + app.create_cursor as u16 + 1,
            input_area.y + 1,
        ));
    }

    let branch_label = if app.create_checkout_existing {
        "Branch to checkout:"
    } else {
        "Base branch:"
    };

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(branch_label, Style::default().fg(colors::CLAUDE_CREAM)),
            Span::raw(" "),
            Span::styled(
                app.create_from_branch
                    .as_deref()
                    .unwrap_or(if app.create_checkout_existing {
                        "(select branch)"
                    } else {
                        "HEAD (current)"
                    }),
                Style::default().fg(colors::CLAUDE_ORANGE),
            ),
        ])),
        Rect::new(inner.x, label_y + 6, inner.width, 1),
    );

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("b", Style::default().fg(colors::CLAUDE_ORANGE)),
            Span::styled(" mode  ", Style::default().fg(colors::CLAUDE_WARM_GRAY)),
            Span::styled("Tab", Style::default().fg(colors::CLAUDE_ORANGE)),
            Span::styled(" branch  ", Style::default().fg(colors::CLAUDE_WARM_GRAY)),
            Span::styled("Enter", Style::default().fg(colors::CLAUDE_ORANGE)),
            Span::styled(" create  ", Style::default().fg(colors::CLAUDE_WARM_GRAY)),
            Span::styled("Esc", Style::default().fg(colors::CLAUDE_ORANGE)),
            Span::styled(" cancel", Style::default().fg(colors::CLAUDE_WARM_GRAY)),
        ]))
        .alignment(Alignment::Center),
        Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1),
    );
}

fn render_branch_select_dialog(frame: &mut Frame, app: &mut App, title: &str) {
    let area = centered_rect(40, 50, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(Line::from(vec![
            Span::raw(" "),
            Span::styled(title, Style::default().fg(colors::CLAUDE_ORANGE).bold()),
            Span::raw(" "),
        ]))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(colors::CLAUDE_ORANGE))
        .style(Style::default().bg(colors::CLAUDE_DARKER))
        .padding(Padding::new(1, 1, 1, 1));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let items: Vec<ListItem> = app
        .available_branches
        .iter()
        .map(|b| {
            let style = if b.is_current {
                Style::default().fg(colors::CLAUDE_ORANGE).bold()
            } else if b.is_remote {
                Style::default().fg(colors::INFO)
            } else {
                Style::default().fg(colors::CLAUDE_CREAM)
            };
            let prefix = if b.is_current {
                " "
            } else if b.is_remote {
                " "
            } else {
                "  "
            };
            ListItem::new(Line::from(vec![
                Span::styled(prefix, style),
                Span::styled(&b.name, style),
            ]))
        })
        .collect();

    let list = List::new(items)
        .highlight_style(
            Style::default().bg(colors::SELECTION_BG), // .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(" ");

    frame.render_stateful_widget(list, inner, &mut app.branch_list_state);

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Enter", Style::default().fg(colors::CLAUDE_ORANGE)),
            Span::styled(" select  ", Style::default().fg(colors::CLAUDE_WARM_GRAY)),
            Span::styled("Esc", Style::default().fg(colors::CLAUDE_ORANGE)),
            Span::styled(" cancel", Style::default().fg(colors::CLAUDE_WARM_GRAY)),
        ]))
        .alignment(Alignment::Center),
        Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1),
    );
}

fn render_delete_dialog(frame: &mut Frame, app: &App) {
    let area = centered_rect(50, 25, frame.area());
    frame.render_widget(Clear, area);

    let wt_name = app
        .selected_worktree()
        .and_then(|w| w.branch.clone())
        .unwrap_or_else(|| "this worktree".into());

    let block = Block::default()
        .title(Line::from(vec![
            Span::raw(" "),
            Span::styled("Delete Worktree", Style::default().fg(colors::ERROR).bold()),
            Span::raw(" "),
        ]))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(colors::ERROR))
        .style(Style::default().bg(colors::CLAUDE_DARKER))
        .padding(Padding::new(2, 2, 1, 1));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    frame.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled(
                    "Are you sure you want to delete ",
                    Style::default().fg(colors::CLAUDE_CREAM),
                ),
                Span::styled(&wt_name, Style::default().fg(colors::CLAUDE_ORANGE).bold()),
                Span::styled("?", Style::default().fg(colors::CLAUDE_CREAM)),
            ]),
            Line::raw(""),
            Line::styled(
                "This action cannot be undone.",
                Style::default().fg(colors::CLAUDE_WARM_GRAY).italic(),
            ),
        ])
        .alignment(Alignment::Center),
        Rect::new(inner.x, inner.y + 1, inner.width, 3),
    );

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " y ",
                Style::default().fg(colors::CLAUDE_DARKER).bg(colors::ERROR),
            ),
            Span::styled(" Yes  ", Style::default().fg(colors::CLAUDE_WARM_GRAY)),
            Span::styled(
                " n ",
                Style::default()
                    .fg(colors::CLAUDE_DARKER)
                    .bg(colors::CLAUDE_WARM_GRAY),
            ),
            Span::styled(" No", Style::default().fg(colors::CLAUDE_WARM_GRAY)),
        ]))
        .alignment(Alignment::Center),
        Rect::new(inner.x, inner.y + inner.height - 2, inner.width, 1),
    );
}

fn render_search_bar(frame: &mut Frame, app: &App) {
    let area = Rect::new(
        frame.area().x + 1,
        frame.area().height - 4,
        frame.area().width - 2,
        3,
    );
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(Line::from(vec![
            Span::raw(" "),
            Span::styled(" Search", Style::default().fg(colors::CLAUDE_ORANGE).bold()),
            Span::raw(" "),
        ]))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(colors::CLAUDE_ORANGE))
        .style(Style::default().bg(colors::CLAUDE_DARKER));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    frame.render_widget(
        Paragraph::new(format!(
            "{}  ({} matches)",
            app.search_query,
            app.filtered_indices.len()
        ))
        .style(Style::default().fg(colors::CLAUDE_CREAM)),
        inner,
    );

    frame.set_cursor_position((inner.x + app.search_cursor as u16, inner.y));
}

// ============================================================================
// Utilities
// ============================================================================

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

fn truncate_str(s: &str, max_len: usize) -> String {
    if s.width() <= max_len {
        s.to_string()
    } else {
        let mut result = String::new();
        let mut width = 0;
        for c in s.chars() {
            let char_width = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
            if width + char_width + 3 > max_len {
                result.push_str("...");
                break;
            }
            result.push(c);
            width += char_width;
        }
        result
    }
}

fn truncate_path(path: &PathBuf, max_len: usize) -> String {
    let s = path.to_string_lossy();
    let width = s.width();
    if width <= max_len {
        s.to_string()
    } else {
        let mut result = String::new();
        let mut current_width = 3; // for "..."
        for c in s.chars().rev() {
            let char_width = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
            if current_width + char_width > max_len {
                break;
            }
            result.push(c);
            current_width += char_width;
        }
        format!("...{}", result.chars().rev().collect::<String>())
    }
}

// ============================================================================
// Main
// ============================================================================

fn main() -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let app_result = App::new();

    let result = match app_result {
        Ok(mut app) => run_app(&mut terminal, &mut app),
        Err(e) => {
            disable_raw_mode()?;
            execute!(
                terminal.backend_mut(),
                LeaveAlternateScreen,
                DisableMouseCapture
            )?;
            terminal.show_cursor()?;
            eprintln!("Error: {}", e);
            eprintln!("\nMake sure you're running this from within a Git repository.");
            return Err(e);
        }
    };

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    if let Err(err) = result {
        eprintln!("Error: {:?}", err);
    }
    Ok(())
}

fn run_app(terminal: &mut Terminal<CrosstermBackend<Stdout>>, app: &mut App) -> Result<()> {
    loop {
        terminal.draw(|f| ui(f, app))?;
        if handle_events(app)? {
            return Ok(());
        }
    }
}
