//! Git Worktree TUI - A beautiful terminal interface for managing Git worktrees
//! Designed with Claude's visual aesthetic: warm tones, clean typography, intuitive interactions

mod cache;

use anyhow::{Context, Result};
use gix::bstr::ByteSlice;
use crossterm::{
    event::{
        DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers, MouseButton,
        MouseEventKind, EventStream,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use futures::StreamExt;
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Margin, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Cell, Clear, List, ListItem, ListState, Padding, Paragraph,
        Row, Scrollbar, ScrollbarOrientation, ScrollbarState, Table, TableState, Wrap,
    },
    Frame, Terminal,
};
use std::{
    fs::File,
    io::{self, Stdout, Write},
    path::PathBuf,
    process::Command,
    time::{Duration, Instant},
};
use tokio::sync::mpsc;
use unicode_width::UnicodeWidthStr;
use tracing::{info, info_span};
use tracing_subscriber::{fmt::{self}, prelude::*, EnvFilter};

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
    pub const WARNING: Color = Color::Rgb(235, 200, 142);
    pub const ERROR: Color = Color::Rgb(248, 113, 113);
    pub const INFO: Color = Color::Rgb(147, 197, 253);
    pub const PURPLE: Color = Color::Rgb(196, 181, 253);

    // UI elements
    pub const BORDER_ACTIVE: Color = CLAUDE_ORANGE;
    pub const BORDER_INACTIVE: Color = Color::Rgb(68, 64, 60);
    pub const SELECTION_BG: Color = Color::Rgb(34, 30, 26);
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
    is_current: bool,
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
            parts.push(format!("↑{}", self.ahead));
        }
        if self.behind > 0 {
            parts.push(format!("↓{}", self.behind));
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
    Error,
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

/// Loading state for async background refresh
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoadingState {
    Idle,
    Loading,
}

/// Message sent from background refresh task
#[derive(Debug)]
enum AppUpdate {
    WorktreesLoaded(Vec<Worktree>),
}

struct App {
    // Core state
    worktrees: Vec<Worktree>,
    table_state: TableState,
    mode: AppMode,
    should_quit: bool,
    cd_path: Option<PathBuf>, // Path to change to on exit (for shell integration)

    // Repository info
    repo_root: PathBuf,
    repo_name: String,
    current_worktree_path: PathBuf,

    // UI state
    status_message: Option<StatusMessage>,
    sort_order: SortOrder,
    show_recent_commits: bool,

    // Loading state for async refresh
    loading_state: LoadingState,
    spinner_frame: usize,

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

    // Error dialog
    error_message: String,

    // Search
    search_query: String,
    search_cursor: usize,
    filtered_indices: Vec<usize>,

    // Git Repository
    repo: gix::Repository,

    // Cached data
    last_refresh: Instant,

    // Mouse support
    list_area: Option<Rect>,
}

impl App {
    fn new() -> Result<Self> {
        let _span = info_span!("App::new").entered();
        let repo = Self::find_git_repository()?;
        
        let repo_root = repo.common_dir().parent().map(|p| p.to_path_buf()).unwrap_or_else(|| repo.common_dir().to_path_buf());
        
        let repo_name = repo_root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "repository".to_string());

        // Get the current worktree path (where the program was run from)
        let current_worktree_path = std::env::current_dir()
            .ok()
            .and_then(|p| dunce::canonicalize(p).ok())
            .unwrap_or_else(|| repo_root.clone());

        // Try to load from cache for instant startup
        let (worktrees, loading_state): (Vec<Worktree>, LoadingState) = if let Some(cached) = cache::load_cache(&repo_root) {
            let is_fresh = cached.is_fresh();
            let worktrees = Self::worktrees_from_cache(cached.worktrees, &repo_root, &current_worktree_path);
            if is_fresh {
                info!(count = worktrees.len(), "Cache hit (fresh)");
                (worktrees, LoadingState::Idle)
            } else {
                info!(count = worktrees.len(), "Cache hit (stale), triggering background refresh");
                (worktrees, LoadingState::Loading)
            }
        } else {
            info!("Cache miss, triggering background load");
            (Vec::new(), LoadingState::Loading)
        };

        let mut app = Self {
            worktrees,
            table_state: TableState::default(),
            mode: AppMode::Normal,
            should_quit: false,
            cd_path: None,

            repo_root,
            repo_name,
            current_worktree_path,

            status_message: None,
            sort_order: SortOrder::Recent,
            show_recent_commits: true,

            loading_state,
            spinner_frame: 0,

            create_input: String::new(),
            create_cursor: 0,
            available_branches: Vec::new(),
            branch_list_state: ListState::default(),
            create_from_branch: None,
            create_checkout_existing: false,
            merge_source_idx: None,

            delete_confirm: false,

            error_message: String::new(),

            search_query: String::new(),
            search_cursor: 0,
            filtered_indices: Vec::new(),

            repo,

            last_refresh: Instant::now(),

            list_area: None,
        };

        // Apply sorting to cached data
        if !app.worktrees.is_empty() {
            app.apply_sort();
            app.filtered_indices = (0..app.worktrees.len()).collect();
            app.table_state.select(Some(0));
        }

        Ok(app)
    }

    /// Convert cached worktrees back to Worktree structs
    fn worktrees_from_cache(
        cached: Vec<cache::CachedWorktree>,
        repo_root: &PathBuf,
        current_path: &PathBuf,
    ) -> Vec<Worktree> {
        cached
            .into_iter()
            .map(|c| {
                let is_main = c.path == *repo_root;
                let is_current = current_path.starts_with(&c.path);
                Worktree {
                    path: c.path,
                    branch: c.branch,
                    commit: c.commit,
                    commit_short: c.commit_short,
                    commit_message: c.commit_message,
                    commit_time: c.commit_time,
                    is_main,
                    is_current,
                    is_bare: c.is_bare,
                    is_detached: c.is_detached,
                    is_locked: c.is_locked,
                    lock_reason: c.lock_reason,
                    is_prunable: c.is_prunable,
                    status: WorktreeStatus {
                        modified: c.status.modified,
                        staged: c.status.staged,
                        untracked: c.status.untracked,
                        ahead: c.status.ahead,
                        behind: c.status.behind,
                    },
                    recent_commits: c
                        .recent_commits
                        .into_iter()
                        .map(|ci| CommitInfo {
                            hash: ci.hash,
                            message: ci.message,
                            time_ago: ci.time_ago,
                        })
                        .collect(),
                }
            })
            .collect()
    }

    /// Convert worktrees to cached format and save to disk
    fn save_to_cache(&self) {
        let cached_worktrees: Vec<cache::CachedWorktree> = self
            .worktrees
            .iter()
            .map(|w| cache::CachedWorktree {
                path: w.path.clone(),
                branch: w.branch.clone(),
                commit: w.commit.clone(),
                commit_short: w.commit_short.clone(),
                commit_message: w.commit_message.clone(),
                commit_time: w.commit_time,
                is_main: w.is_main,
                is_current: w.is_current,
                is_bare: w.is_bare,
                is_detached: w.is_detached,
                is_locked: w.is_locked,
                lock_reason: w.lock_reason.clone(),
                is_prunable: w.is_prunable,
                status: cache::CachedWorktreeStatus {
                    modified: w.status.modified,
                    staged: w.status.staged,
                    untracked: w.status.untracked,
                    ahead: w.status.ahead,
                    behind: w.status.behind,
                },
                recent_commits: w
                    .recent_commits
                    .iter()
                    .map(|ci| cache::CachedCommitInfo {
                        hash: ci.hash.clone(),
                        message: ci.message.clone(),
                        time_ago: ci.time_ago.clone(),
                    })
                    .collect(),
            })
            .collect();

        let cache_data = cache::create_cache(self.repo_root.clone(), cached_worktrees);
        let _ = cache::save_cache(&cache_data);
    }

    fn find_git_repository() -> Result<gix::Repository> {
        let repo = gix::discover(".").context("Failed to find a git repository")?;
        Ok(repo)
    }

    fn refresh_worktrees(&mut self) -> Result<()> {
        let _span = info_span!("refresh_worktrees").entered();
        info!("Synchronous worktree refresh started");
        
        let worktree_proxies = self.repo.worktrees().context("Failed to list worktrees")?;
        let mut worktrees = Vec::new();

        // Add main worktree
        worktrees.push(self.create_worktree_info(None)?);

        // Add linked worktrees
        for proxy in worktree_proxies {
            worktrees.push(self.create_worktree_info(Some(proxy))?);
        }

        self.worktrees = worktrees;
        self.last_refresh = Instant::now();

        // Fetch additional status for each worktree
        for worktree in &mut self.worktrees {
            if !worktree.is_bare {
                let repo = gix::open(&worktree.path).context("Failed to open worktree repo")?;
                let status = Self::get_gix_status(&repo)?;
                worktree.status = status;

                let commit_info = Self::get_gix_commit_info(&repo)?;
                worktree.commit_message = commit_info.0;
                worktree.commit_time = commit_info.1;
                worktree.recent_commits = Self::get_gix_recent_commits(&repo, 10)?;
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

        // Save to cache
        self.save_to_cache();
        
        self.loading_state = LoadingState::Idle;
        self.set_status("Refreshed worktree list", MessageLevel::Info);
        Ok(())
    }

    fn create_worktree_info(&self, proxy: Option<gix::worktree::Proxy<'_>>) -> Result<Worktree> {
        let (path, branch, commit, is_main, is_locked, lock_reason) = match proxy {
            Some(p) => {
                let path = p.base()?.to_path_buf();
                let is_locked = p.lock_reason().is_some();
                let lock_reason = p.lock_reason().map(|s| s.to_string());
                // To get branch and commit, we need to open the repo at that path or use the proxy
                let wt_repo = p.into_repo().context("Failed to open worktree repo from proxy")?;
                let head = wt_repo.head().context("Failed to get HEAD")?;
                let branch = head.referent_name().map(|n| n.shorten().to_string());
                let commit = head.id().map(|id| id.to_string()).unwrap_or_default();
                (path, branch, commit, false, is_locked, lock_reason)
            }
            None => {
                let path = self.repo.work_dir().map(|p| p.to_path_buf()).unwrap_or_else(|| self.repo.common_dir().to_path_buf());
                let head = self.repo.head().context("Failed to get HEAD")?;
                let branch = head.referent_name().map(|n| n.shorten().to_string());
                let commit = head.id().map(|id| id.to_string()).unwrap_or_default();
                (path, branch, commit, true, false, None)
            }
        };

        let current_dir = std::env::current_dir()?;
        let is_current = current_dir.starts_with(&path);

        Ok(Worktree {
            path: path.clone(),
            branch,
            commit_short: commit.chars().take(7).collect::<String>(),
            commit,
            commit_message: String::new(),
            commit_time: None,
            is_main,
            is_current,
            is_bare: self.repo.is_bare() && is_main,
            is_detached: false, // Will be set by head info if needed
            is_locked,
            lock_reason,
            is_prunable: !path.exists(),
            status: WorktreeStatus::default(),
            recent_commits: Vec::new(),
        })
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

    fn get_gix_status(repo: &gix::Repository) -> Result<WorktreeStatus> {
        let mut status = WorktreeStatus::default();
        if repo.is_bare() {
            return Ok(status);
        }

        // Use high-level status API
        if let Ok(stat) = repo.status(gix::progress::Discard) {
            if let Ok(res) = stat.index_worktree_rewrites(None)
                .into_index_worktree_iter(Vec::<gix::bstr::BString>::new()) {
                for item in res {
                    if let Ok(item) = item {
                        match item {
                            gix::status::index_worktree::Item::Modification { .. } => status.modified += 1,
                            _ => {}
                        }
                    }
                }
            }
        }

        // Ahead/Behind - Placeholder for now
        status.ahead = 0;
        status.behind = 0;

        Ok(status)
    }

    fn get_gix_commit_info(repo: &gix::Repository) -> Result<(String, Option<i64>)> {
        let head = repo.head()?;
        if let Some(id) = head.id() {
            let commit = repo.find_object(id)?.into_commit();
            let message = commit.message()?.summary().to_string();
            let time = commit.time()?.seconds;
            Ok((message, Some(time as i64)))
        } else {
            Ok((String::new(), None))
        }
    }

    fn get_gix_recent_commits(repo: &gix::Repository, count: usize) -> Result<Vec<CommitInfo>> {
        let mut commits = Vec::new();
        let head = repo.head()?;
        if let Some(id) = head.id() {
            let walk = repo.rev_walk([id.detach()]).all()?;
            for (i, commit_info) in walk.enumerate() {
                if i >= count { break; }
                let commit_info = commit_info?;
                let commit = repo.find_object(commit_info.id)?.into_commit();
                let message = commit.message()?.summary().to_string();
                let hash = commit.id().to_string().chars().take(7).collect::<String>();
                
                let time = commit.time()?;
                let now = gix::date::Time::now_local_or_utc();
                let diff_secs = now.seconds.saturating_sub(time.seconds);
                let time_ago = if diff_secs < 60 {
                    format!("{}s ago", diff_secs)
                } else if diff_secs < 3600 {
                    format!("{}m ago", diff_secs / 60)
                } else if diff_secs < 86400 {
                    format!("{}h ago", diff_secs / 3600)
                } else {
                    format!("{}d ago", diff_secs / 86400)
                };

                commits.push(CommitInfo {
                    hash,
                    message,
                    time_ago,
                });
            }
        }
        Ok(commits)
    }

    fn refresh_branches(&mut self) -> Result<()> {
        let mut branches = Vec::new();

        let refs = self.repo.references()?;
        let head_name = self.repo.head()?.referent_name().map(|n| n.as_bstr().to_string()).unwrap_or_default();

        if let Ok(local_branches) = refs.local_branches() {
            for head in local_branches {
                let head = head.map_err(|e| anyhow::anyhow!("{}", e))?;
                branches.push(Branch {
                    name: head.name().shorten().to_string(),
                    is_remote: false,
                    is_current: head.name().as_bstr().to_string() == head_name,
                });
            }
        }

        if let Ok(remote_branches) = refs.remote_branches() {
            for remote_ref in remote_branches {
                let remote_ref = remote_ref.map_err(|e| anyhow::anyhow!("{}", e))?;
                let name = remote_ref.name().shorten().to_string();
                if !name.contains("HEAD") {
                    branches.push(Branch {
                        name,
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
        // If it's an error, also show it in a popup
        if level == MessageLevel::Error {
            self.error_message = text.to_string();
            self.mode = AppMode::Error;
        }
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
        // repo_root is now guaranteed to be absolute
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
            // Only clear mode and input on success
            self.mode = AppMode::Normal;
            self.create_input.clear();
            self.create_cursor = 0;
            self.create_from_branch = None;
            self.create_checkout_existing = false;
            // get index of newly created worktree and select it
            // Assumes worktree was created successfully
            if let Some(pos) = self
                .worktrees
                .iter()
                .position(|wt| wt.path == worktree_path)
            {
                if let Some(filtered_pos) = self.filtered_indices.iter().position(|&idx| idx == pos)
                {
                    self.table_state.select(Some(filtered_pos));
                }
            }
        } else {
            let error = String::from_utf8_lossy(&output.stderr);
            self.set_status(&format!("Failed: {}", error.trim()), MessageLevel::Error);
            // Don't reset mode - keep error dialog open
        }
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
                // Only clear mode on success
                self.mode = AppMode::Normal;
                self.delete_confirm = false;
            } else {
                let error = String::from_utf8_lossy(&output.stderr);
                self.set_status(&format!("Failed: {}", error.trim()), MessageLevel::Error);
                // Don't reset mode - keep error dialog open
            }
        }
        Ok(())
    }

    fn copy_path_to_clipboard(&mut self) {
        if let Some(wt) = self.selected_worktree() {
            let path = wt.path.to_string_lossy().to_string();
            self.copy_text_to_clipboard(&path);
            self.set_status(&format!("Copied: {}", path), MessageLevel::Success);
        }
    }

    fn copy_text_to_clipboard(&mut self, text: &str) {
        #[cfg(target_os = "macos")]
        let result = Command::new("pbcopy")
            .stdin(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                use std::io::Write;
                if let Some(ref mut stdin) = child.stdin {
                    stdin.write_all(text.as_bytes())?;
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
                    stdin.write_all(text.as_bytes())?;
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
                    stdin.write_all(text.as_bytes())?;
                }
                child.wait()
            });

        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        let result: Result<std::process::ExitStatus, std::io::Error> = Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            "Clipboard not supported",
        ));

        match result {
            Ok(_) => {}
            Err(_) => self.set_status("Failed to copy to clipboard", MessageLevel::Error),
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
        // Try to detect the main branch name from origin/HEAD
        if let Ok(remote_head) = self.repo.find_reference("refs/remotes/origin/HEAD") {
            if let Some(Ok(target)) = remote_head.follow() {
                if let Some(name) = target.name().shorten().to_str().ok().and_then(|s| s.strip_prefix("origin/")) {
                    return name.to_string();
                }
            }
        }

        // Fallback: check if main or master exists
        if self.repo.find_reference("refs/heads/main").is_ok() {
            return "main".to_string();
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
        // keep selection on the same worktree if possible
        // get currently selected worktrees name
        let selected_wt_name = self.selected_worktree().and_then(|wt| wt.branch.clone());
        self.apply_sort();
        self.filtered_indices = (0..self.worktrees.len()).collect();
        // restore selection
        if let Some(name) = selected_wt_name {
            if let Some(pos) = self
                .worktrees
                .iter()
                .position(|wt| wt.branch.as_ref() == Some(&name))
            {
                if let Some(filtered_pos) = self.filtered_indices.iter().position(|&idx| idx == pos)
                {
                    self.table_state.select(Some(filtered_pos));
                }
            }
        }
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

        KeyCode::Tab => {}

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

        // Change directory to selected worktree (for shell integration)
        KeyCode::Char(' ') => {
            if let Some(wt) = app.selected_worktree() {
                app.cd_path = Some(wt.path.clone());
                app.should_quit = true;
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

fn handle_error_mode(app: &mut App, key: KeyCode) -> Result<()> {
    match key {
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Enter => {
            app.mode = AppMode::Normal;
            app.error_message.clear();
        }
        KeyCode::Char('y') => {
            let error = app.error_message.clone();
            app.copy_text_to_clipboard(&error);
            app.mode = AppMode::Normal;
            app.error_message.clear();
            app.set_status("Error copied to clipboard", MessageLevel::Success);
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
        KeyCode::BackTab => {
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
        AppMode::Error => render_error_dialog(frame, app),
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
        Span::styled("Worktree TUI", Style::default().fg(colors::CLAUDE_CREAM)),
        Span::raw(" "),
        Span::styled(":: ", Style::default().fg(colors::CLAUDE_WARM_GRAY)),
        Span::styled(
            &app.repo_name,
            Style::default().fg(colors::CLAUDE_WARM_GRAY),
        ),
    ]);
    frame.render_widget(Paragraph::new(logo), header_layout[0]);

    // let total = app.worktrees.len();
    // let dirty = app
    //     .worktrees
    //     .iter()
    //     .filter(|w| !w.status.is_clean())
    //     .count();

    // let mut stats_spans = vec![Span::styled(
    //     format!("? untracked ~ modified + staged"),
    //     Style::default().fg(colors::CLAUDE_WARM_GRAY),
    // )];
    let mut stats_spans = vec![
        Span::styled(format!(" ? "), Style::default().fg(colors::CLAUDE_CREAM)),
        Span::styled(
            format!("untracked "),
            Style::default().fg(colors::CLAUDE_WARM_GRAY),
        ),
        Span::styled(format!(" ~ "), Style::default().fg(colors::CLAUDE_CREAM)),
        Span::styled(
            format!("modified "),
            Style::default().fg(colors::CLAUDE_WARM_GRAY),
        ),
        Span::styled(format!(" + "), Style::default().fg(colors::CLAUDE_CREAM)),
        Span::styled(
            format!("staged "),
            Style::default().fg(colors::CLAUDE_WARM_GRAY),
        ),
    ];

    // let mut stats_spans = vec![Span::styled(
    //     format!("{} worktrees", total),
    //     Style::default().fg(colors::CLAUDE_WARM_GRAY),
    // )];
    // if dirty > 0 {
    //     stats_spans.push(Span::styled(
    //         format!("  {} dirty", dirty),
    //         Style::default().fg(colors::WARNING),
    //     ));
    // }

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
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    render_worktree_list(frame, app, content_chunks[0]);
    render_details_panel(frame, app, content_chunks[1]);
}

fn render_worktree_list(frame: &mut Frame, app: &mut App, area: Rect) {
    app.list_area = Some(area);

    let border_color = colors::BORDER_INACTIVE;

    let block = Block::default()
        .title(Line::from(vec![
            Span::raw(" "),
            Span::styled("Worktrees", Style::default().fg(colors::CLAUDE_CREAM)),
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
            // get the main worktree too if not main already
            let main_wt = if wt.is_main {
                Some(wt)
            } else {
                app.worktrees.iter().find(|wt| wt.is_main)
            };

            if main_wt.is_none() {
                app.error_message = "No main worktree found!".into();
                app.mode = AppMode::Error;
            }

            let num = if display_idx < 9 {
                Span::styled(
                    format!("{}", display_idx + 1),
                    Style::default().fg(colors::CLAUDE_WARM_GRAY),
                )
            } else {
                Span::raw(" ")
            };

            let icon = if wt.is_current {
                // Highlight the worktree we're currently in
                Span::styled("*", Style::default().fg(colors::CLAUDE_CREAM)) // other ones: ○ 
            } else if wt.is_main {
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

            // make commits in table that are matching the main one highlight in purple
            let commit_style = if wt.is_main {
                Style::default().fg(colors::PURPLE)
            } else {
                if main_wt.is_none() {
                    Style::default().fg(colors::CLAUDE_WARM_GRAY)
                } else {
                    if wt.commit == main_wt.unwrap().commit {
                        Style::default().fg(colors::PURPLE)
                    } else {
                        Style::default().fg(colors::CLAUDE_WARM_GRAY)
                    }
                }
            };

            Row::new(vec![
                Cell::from(num),
                Cell::from(icon),
                Cell::from(Span::styled(branch_name, branch_style)),
                Cell::from(Span::styled(wt.status.summary(), status_style)),
                Cell::from(Span::styled(&wt.commit_short, commit_style)),
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
        .highlight_symbol(Span::styled(
            "→ ",
            Style::default().fg(colors::CLAUDE_WARM_GRAY),
        ));

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
    let border_color = colors::BORDER_INACTIVE;

    let block = Block::default()
        .title(Line::from(vec![
            Span::raw(" "),
            Span::styled("Details", Style::default().fg(colors::CLAUDE_CREAM)),
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
        let branch_name = wt.branch.as_deref().unwrap_or(if wt.is_detached {
            "(detached)"
        } else {
            "(bare)"
        });
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
            status_spans.push(Span::styled(
                "Modified",
                Style::default().fg(colors::WARNING),
            ));
            status_spans.push(Span::raw(" "));
            let mut parts = Vec::new();
            if wt.status.staged > 0 {
                parts.push(Span::styled(
                    format!("+{}", wt.status.staged),
                    Style::default().fg(colors::SUCCESS),
                ));
            }
            if wt.status.modified > 0 {
                parts.push(Span::styled(
                    format!("~{}", wt.status.modified),
                    Style::default().fg(colors::WARNING),
                ));
            }
            if wt.status.untracked > 0 {
                parts.push(Span::styled(
                    format!("?{}", wt.status.untracked),
                    Style::default().fg(colors::CLAUDE_WARM_GRAY),
                ));
            }

            for (i, part) in parts.into_iter().enumerate() {
                if i > 0 {
                    status_spans.push(Span::raw(" "));
                }
                status_spans.push(part);
            }
            // status_spans.push(Span::raw(")"));
        }

        if wt.status.ahead > 0 || wt.status.behind > 0 {
            status_spans.push(Span::styled(
                " • ",
                Style::default().fg(colors::CLAUDE_WARM_GRAY),
            ));
            if wt.status.ahead > 0 {
                status_spans.push(Span::styled(
                    format!("↑{}", wt.status.ahead),
                    Style::default().fg(colors::SUCCESS),
                ));
                if wt.status.behind > 0 {
                    status_spans.push(Span::raw(" "));
                }
            }
            if wt.status.behind > 0 {
                status_spans.push(Span::styled(
                    format!("↓{}", wt.status.behind),
                    Style::default().fg(colors::ERROR),
                ));
            }
        }
        lines.push(Line::from(status_spans));
        lines.push(Line::raw(""));

        // --- Location ---
        lines.push(Line::from(Span::styled(
            "Location",
            Style::default().fg(colors::CLAUDE_WARM_GRAY),
        )));
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                truncate_path(&wt.path, inner.width.saturating_sub(4) as usize),
                Style::default().fg(colors::CLAUDE_CREAM),
            ),
        ]));
        lines.push(Line::raw(""));

        // --- Current Commit ---
        lines.push(Line::from(Span::styled(
            "Current Commit",
            Style::default().fg(colors::CLAUDE_WARM_GRAY),
        )));
        let time_ago = wt
            .recent_commits
            .first()
            .map(|c| c.time_ago.clone())
            .unwrap_or_default();
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(&wt.commit_short, Style::default().fg(colors::INFO)),
            Span::styled(
                format!(" • {}", time_ago),
                Style::default().fg(colors::CLAUDE_WARM_GRAY).italic(),
            ),
        ]));

        if !wt.commit_message.is_empty() {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    &wt.commit_message,
                    Style::default().fg(colors::CLAUDE_CREAM).italic(),
                ),
            ]));
        }
        lines.push(Line::raw(""));

        // --- Attributes ---
        if wt.is_locked || wt.is_prunable {
            lines.push(Line::from(Span::styled(
                "Attributes",
                Style::default().fg(colors::CLAUDE_WARM_GRAY),
            )));
            if wt.is_locked {
                lines.push(Line::from(vec![
                    Span::raw("  Locked: "),
                    Span::styled(
                        wt.lock_reason.as_deref().unwrap_or("no reason provided"),
                        Style::default().fg(colors::WARNING).italic(),
                    ),
                ]));
            }
            if wt.is_prunable {
                lines.push(Line::from(vec![
                    Span::raw("  Prunable: "),
                    Span::styled(
                        "Worktree path is missing or invalid",
                        Style::default().fg(colors::ERROR).italic(),
                    ),
                ]));
            }
            lines.push(Line::raw(""));
        }

        // --- History ---
        if app.show_recent_commits && wt.recent_commits.len() > 1 {
            lines.push(Line::from(vec![
                Span::styled(
                    "Recent History",
                    Style::default().fg(colors::CLAUDE_WARM_GRAY),
                ),
                Span::styled(
                    " (t to toggle)",
                    Style::default().fg(colors::CLAUDE_WARM_GRAY).italic(),
                ),
            ]));

            for commit in wt.recent_commits.iter().skip(1).take(8) {
                let msg = truncate_str(&commit.message, inner.width.saturating_sub(16) as usize);
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("  {} ", commit.hash),
                        Style::default().fg(colors::PURPLE),
                    ),
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
            ("p/P", "pull/push"),
            ("x", "delete"),
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

    // Build right side content: spinner (if loading) + status message
    let mut right_spans: Vec<Span> = Vec::new();
    
    // Add spinner if loading
    if app.loading_state == LoadingState::Loading {
        let spinner_char = SPINNER_FRAMES[app.spinner_frame];
        right_spans.push(Span::styled(
            format!("{} ", spinner_char),
            Style::default().fg(colors::BORDER_INACTIVE),
        ));
    }

    if let Some(ref msg) = app.status_message {
        let color = match msg.level {
            MessageLevel::Info => colors::INFO,
            MessageLevel::Success => colors::SUCCESS,
            MessageLevel::Warning => colors::WARNING,
            MessageLevel::Error => colors::ERROR,
        };
        right_spans.push(Span::styled(&msg.text, Style::default().fg(color)));
    }
    
    if !right_spans.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(right_spans)).alignment(Alignment::Right),
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
                "Shift+Tab        Toggle new/existing branch",
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
                "Space            Change to worktree dir",
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
            Span::styled(
                "(Shift+Tab to toggle)",
                Style::default().fg(colors::CLAUDE_WARM_GRAY).italic(),
            ),
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
        Paragraph::new(Span::styled(
            label,
            Style::default().fg(colors::CLAUDE_CREAM),
        )),
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
            Span::styled("Shift+Tab", Style::default().fg(colors::CLAUDE_ORANGE)),
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

fn render_error_dialog(frame: &mut Frame, app: &App) {
    let area = centered_rect(60, 40, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(Line::from(vec![
            Span::raw(" "),
            Span::styled("Error", Style::default().fg(colors::ERROR).bold()),
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
            Line::from(Span::styled(
                "An error occurred:",
                Style::default().fg(colors::CLAUDE_WARM_GRAY),
            )),
            Line::raw(""),
            Line::from(Span::styled(
                &app.error_message,
                Style::default().fg(colors::CLAUDE_CREAM),
            )),
        ])
        .wrap(Wrap { trim: false }),
        Rect::new(inner.x, inner.y, inner.width, inner.height - 3),
    );

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("y", Style::default().fg(colors::CLAUDE_ORANGE)),
            Span::styled(" copy  ", Style::default().fg(colors::CLAUDE_WARM_GRAY)),
            Span::styled("Enter", Style::default().fg(colors::CLAUDE_ORANGE)),
            Span::styled(" close  ", Style::default().fg(colors::CLAUDE_WARM_GRAY)),
            Span::styled("Esc", Style::default().fg(colors::CLAUDE_ORANGE)),
            Span::styled(" close", Style::default().fg(colors::CLAUDE_WARM_GRAY)),
        ]))
        .alignment(Alignment::Center),
        Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1),
    );
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

/// Spinner characters for loading indicator
const SPINNER_FRAMES: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

#[derive(Debug, Clone, Copy, Eq, PartialEq, Default)]
pub struct JustTime;

impl tracing_subscriber::fmt::time::FormatTime for JustTime {
    fn format_time(&self, w: &mut tracing_subscriber::fmt::format::Writer<'_>) -> std::fmt::Result {
        write!(w, "{}", chrono::Local::now().format("%H:%M:%S"))
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/wtt.log")
        .expect("Failed to open log file");
        
    let (non_blocking, _guard) = tracing_appender::non_blocking(log_file);
    
    tracing_subscriber::registry()
        .with(EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into()))
        .with(fmt::layer()
            .with_writer(non_blocking)
            .with_ansi(false)
            .compact()
            .with_timer(JustTime)
            .with_span_events(fmt::format::FmtSpan::CLOSE))
        .init();

    info!("Starting worktree-tui");
    // Parse --cwd-file argument (for shell integration)
    let cwd_file: Option<PathBuf> = std::env::args()
        .skip(1)
        .find(|arg| arg.starts_with("--cwd-file="))
        .map(|arg| PathBuf::from(arg.strip_prefix("--cwd-file=").unwrap()));

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let app_result = App::new();

    let result = match app_result {
        Ok(mut app) => run_app(&mut terminal, &mut app).await,
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

    match result {
        Ok(Some(cd_path)) => {
            // Write path to cwd-file for shell integration
            if let Some(ref file_path) = cwd_file {
                if let Ok(mut file) = File::create(file_path) {
                    let _ = writeln!(file, "{}", cd_path.display());
                }
            }
        }
        Ok(None) => {}
        Err(err) => {
            eprintln!("Error: {:?}", err);
        }
    }
    Ok(())
}

async fn run_app(terminal: &mut Terminal<CrosstermBackend<Stdout>>, app: &mut App) -> Result<Option<PathBuf>> {
    // Create channel for background refresh updates
    let (tx, mut rx) = mpsc::unbounded_channel::<AppUpdate>();
    
    // If we need to load/refresh, spawn background task
    if app.loading_state == LoadingState::Loading {
        spawn_refresh_task(tx.clone(), app.repo_root.clone(), app.current_worktree_path.clone());
    }
    
    // Create async event stream
    let mut event_stream = EventStream::new();
    
    // Spinner tick interval (100ms for smooth animation)
    let mut spinner_interval = tokio::time::interval(Duration::from_millis(100));
    
    loop {
        // Render
        terminal.draw(|f| ui(f, app))?;
        
        // Async event handling with tokio::select!
        tokio::select! {
            // Handle keyboard/mouse events
            maybe_event = event_stream.next() => {
                if let Some(Ok(event)) = maybe_event {
                    if handle_event(app, event, &tx)? {
                        return Ok(app.cd_path.take());
                    }
                }
            }
            
            // Handle background refresh updates
            Some(update) = rx.recv() => {
                match update {
                    AppUpdate::WorktreesLoaded(worktrees) => {
                        let selected = app.table_state.selected();
                        app.worktrees = worktrees;
                        app.apply_sort();
                        app.filtered_indices = (0..app.worktrees.len()).collect();
                        app.loading_state = LoadingState::Idle;
                        app.save_to_cache();
                        
                        // Restore selection
                        if let Some(idx) = selected {
                            if idx < app.filtered_indices.len() {
                                app.table_state.select(Some(idx));
                            } else if !app.filtered_indices.is_empty() {
                                app.table_state.select(Some(0));
                            }
                        } else if !app.filtered_indices.is_empty() {
                            app.table_state.select(Some(0));
                        }
                        
                        app.set_status("Refreshed from background", MessageLevel::Success);
                    }
                }
            }
            
            // Spinner animation tick
            _ = spinner_interval.tick() => {
                if app.loading_state == LoadingState::Loading {
                    app.spinner_frame = (app.spinner_frame + 1) % SPINNER_FRAMES.len();
                }
                app.clear_old_status();
            }
        }
    }
}

/// Spawn a background task to refresh worktree data
fn spawn_refresh_task(tx: mpsc::UnboundedSender<AppUpdate>, repo_root: PathBuf, current_path: PathBuf) {
    tokio::spawn(async move {
        // Run blocking git commands in a blocking task
        let result = tokio::task::spawn_blocking(move || {
            fetch_all_worktrees(&repo_root, &current_path)
        }).await;
        
        if let Ok(Ok(worktrees)) = result {
            let _ = tx.send(AppUpdate::WorktreesLoaded(worktrees));
        }
    });
}

/// Fetch all worktree data (runs in blocking thread with parallel git commands)
fn fetch_all_worktrees(repo_root: &PathBuf, _current_path: &PathBuf) -> Result<Vec<Worktree>> {
    let _span = info_span!("fetch_all_worktrees").entered();
    info!(repo_root = %repo_root.display(), "Fetching all worktrees");
    
    let repo = gix::open(repo_root).context("Failed to open repository")?;
    let worktree_proxies = repo.worktrees().context("Failed to list worktrees")?;
    
    let mut worktrees = Vec::new();

    // Helper to create Worktree struct (basically create_worktree_info but standalone)
    let get_wt_info = |proxy: Option<gix::worktree::Proxy<'_>>, r: &gix::Repository| -> Result<Worktree> {
        let (path, branch, commit, is_main, is_locked, lock_reason) = match proxy {
            Some(p) => {
                let path = p.base()?.to_path_buf();
                let is_locked = p.lock_reason().is_some();
                let lock_reason = p.lock_reason().map(|s| s.to_string());
                let wt_repo = p.into_repo().context("Failed to open worktree repo")?;
                let head = wt_repo.head().context("Failed to get HEAD")?;
                let branch = head.referent_name().map(|n| n.shorten().to_string());
                let commit = head.id().map(|id| id.to_string()).unwrap_or_default();
                (path, branch, commit, false, is_locked, lock_reason)
            }
            None => {
                let path = r.work_dir().map(|p| p.to_path_buf()).unwrap_or_else(|| r.common_dir().to_path_buf());
                let head = r.head().context("Failed to get HEAD")?;
                let branch = head.referent_name().map(|n| n.shorten().to_string());
                let commit = head.id().map(|id| id.to_string()).unwrap_or_default();
                (path, branch, commit, true, false, None)
            }
        };

        let current_dir = std::env::current_dir()?;
        let is_current = current_dir.starts_with(&path);

        Ok(Worktree {
            path: path.clone(),
            branch,
            commit_short: commit.chars().take(7).collect::<String>(),
            commit,
            commit_message: String::new(),
            commit_time: None,
            is_main,
            is_current,
            is_bare: r.is_bare() && is_main,
            is_detached: false,
            is_locked,
            lock_reason,
            is_prunable: !path.exists(),
            status: WorktreeStatus::default(),
            recent_commits: Vec::new(),
        })
    };

    // Add main worktree
    worktrees.push(get_wt_info(None, &repo)?);

    // Add linked worktrees
    for proxy in worktree_proxies {
        worktrees.push(get_wt_info(Some(proxy), &repo)?);
    }

    // Fetch additional status for each worktree IN PARALLEL
    std::thread::scope(|s| {
        let mut task_handles = Vec::new();
        
        for (i, wt) in worktrees.iter().enumerate() {
            if wt.is_bare || wt.is_prunable { continue; }
            let path = wt.path.clone();
            
            task_handles.push(s.spawn(move || {
                let _span = info_span!("fetch_wt_details", wt_idx = i, path = %path.display()).entered();
                if let Ok(repo) = gix::open(&path) {
                    let status = App::get_gix_status(&repo).unwrap_or_default();
                    let commit_info = App::get_gix_commit_info(&repo).unwrap_or_else(|_| (String::new(), None));
                    let recent_commits = App::get_gix_recent_commits(&repo, 10).unwrap_or_default();
                    (i, status, commit_info, recent_commits)
                } else {
                    (i, WorktreeStatus::default(), (String::new(), None), Vec::new())
                }
            }));
        }
        
        for handle in task_handles {
            if let Ok((idx, status, commit_info, recent_commits)) = handle.join() {
                worktrees[idx].status = status;
                worktrees[idx].commit_message = commit_info.0;
                worktrees[idx].commit_time = commit_info.1;
                worktrees[idx].recent_commits = recent_commits;
            }
        }
    });
    
    Ok(worktrees)
}



/// Handle a single event, return true if should quit
fn handle_event(app: &mut App, event: Event, tx: &mpsc::UnboundedSender<AppUpdate>) -> Result<bool> {
    match event {
        Event::Key(key) => match app.mode {
            AppMode::Normal => handle_normal_mode_async(app, key.code, key.modifiers, tx)?,
            AppMode::Help => handle_help_mode(app, key.code)?,
            AppMode::Create => handle_create_mode(app, key.code, key.modifiers)?,
            AppMode::Delete => handle_delete_mode(app, key.code)?,
            AppMode::Search => handle_search_mode(app, key.code, key.modifiers)?,
            AppMode::BranchSelect => handle_branch_select_mode(app, key.code)?,
            AppMode::MergeSelect => handle_merge_select_mode(app, key.code)?,
            AppMode::Error => handle_error_mode(app, key.code)?,
        },
        Event::Mouse(mouse) => {
            handle_mouse_event(app, mouse)?;
        }
        _ => {}
    }
    Ok(app.should_quit)
}

/// Handle normal mode with async refresh capability
fn handle_normal_mode_async(
    app: &mut App, 
    key: KeyCode, 
    modifiers: KeyModifiers,
    tx: &mpsc::UnboundedSender<AppUpdate>
) -> Result<()> {
    match key {
        // Refresh triggers background task instead of blocking
        KeyCode::Char('r') | KeyCode::Char('R') => {
            if app.loading_state != LoadingState::Loading {
                app.loading_state = LoadingState::Loading;
                spawn_refresh_task(tx.clone(), app.repo_root.clone(), app.current_worktree_path.clone());
                app.set_status("Refreshing...", MessageLevel::Info);
            }
        }
        // All other keys handled by existing function
        _ => handle_normal_mode(app, key, modifiers)?
    }
    Ok(())
}
