impl App {
    fn new() -> Result<Self> {
        let _span = info_span!("App::new").entered();
        let repo = Self::find_git_repository()?;

        // common_dir() points to the SHARED git directory.
        // Opening it directly gives us the "main" repository view.
        let main_repo = gix::open(repo.common_dir())
            .context("Failed to open main repository from common_dir")?;

        let repo_root = main_repo
            .workdir()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| main_repo.path().to_path_buf());

        let repo_name = repo_root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "repository".to_string());

        info!(?repo_root, ?repo_name, "Main repository identified");

        // Get the current worktree path (where the program was run from)
        let current_worktree_path = std::env::current_dir()
            .ok()
            .and_then(|p| dunce::canonicalize(p).ok())
            .unwrap_or_else(|| repo_root.clone());

        info!(?current_worktree_path, "Current directory identified");

        // Try to load from cache for instant startup
        let (worktrees, loading_state): (Vec<Worktree>, LoadingState) =
            if let Some(cached) = cache::load_cache(&repo_root) {
                let is_fresh = cached.is_fresh();
                let worktrees =
                    Self::worktrees_from_cache(cached.worktrees, &repo_root, &current_worktree_path);
                if is_fresh {
                    (worktrees, LoadingState::Idle)
                } else {
                    (worktrees, LoadingState::Loading)
                }
            } else {
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

            repo: main_repo,

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
                let is_current = current_path == &c.path || current_path.starts_with(&c.path);
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

        // Always use the main repo root for listing
        let repo = gix::open(&self.repo_root).context("Failed to open main repository")?;
        let worktree_proxies = repo.worktrees().context("Failed to list worktrees")?;
        let mut worktrees = Vec::new();

        // Helper to create Worktree struct
        let get_wt_info = |proxy: Option<gix::worktree::Proxy<'_>>,
                           r: &gix::Repository|
         -> Result<Worktree> {
            let (path, branch, commit, is_main, is_locked, lock_reason) = match proxy {
                Some(p) => {
                    let path = p.base()?.to_path_buf();
                    let is_locked = p.lock_reason().is_some();
                    let lock_reason = p.lock_reason().map(|s| s.to_string());
                    let wt_repo = p
                        .into_repo()
                        .context("Failed to open worktree repo from proxy")?;
                    let head = wt_repo.head().context("Failed to get HEAD")?;
                    let branch = head.referent_name().map(|n| n.shorten().to_string());
                    let commit = head.id().map(|id| id.to_string()).unwrap_or_default();
                    (path, branch, commit, false, is_locked, lock_reason)
                }
                None => {
                    let path = r
                        .workdir()
                        .map(|p| p.to_path_buf())
                        .unwrap_or_else(|| r.common_dir().to_path_buf());
                    let head = r.head().context("Failed to get HEAD")?;
                    let branch = head.referent_name().map(|n| n.shorten().to_string());
                    let commit = head.id().map(|id| id.to_string()).unwrap_or_default();
                    (path, branch, commit, true, false, None)
                }
            };

            let canon_path = dunce::canonicalize(&path).unwrap_or_else(|_| path.clone());
            let canon_current = dunce::canonicalize(&self.current_worktree_path)
                .unwrap_or_else(|_| self.current_worktree_path.clone());
            let is_current = canon_current == canon_path || canon_current.starts_with(&canon_path);

            Ok(Worktree {
                path: canon_path,
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
            let wt = get_wt_info(Some(proxy), &repo)?;
            // De-duplicate
            if !worktrees.iter().any(|existing| existing.path == wt.path) {
                worktrees.push(wt);
            }
        }

        self.worktrees = worktrees;
        self.last_refresh = Instant::now();

        // Fetch additional status for each worktree
        for worktree in &mut self.worktrees {
            if !worktree.is_bare && !worktree.is_prunable {
                if let Ok(repo) = gix::open(&worktree.path) {
                    worktree.status = Self::get_gix_status(&repo).unwrap_or_default();
                    let commit_info = Self::get_gix_commit_info(&repo)
                        .unwrap_or_else(|_| (String::new(), None));
                    worktree.commit_message = commit_info.0;
                    worktree.commit_time = commit_info.1;
                    worktree.recent_commits =
                        Self::get_gix_recent_commits(&repo, 10).unwrap_or_default();
                }
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

    fn get_gix_status(repo: &gix::Repository) -> Result<WorktreeStatus> {
        let mut status = WorktreeStatus::default();
        if repo.is_bare() {
            return Ok(status);
        }

        // Use high-level status API
        if let Ok(stat) = repo.status(gix::progress::Discard) {
            // Index to Worktree changes
            if let Ok(res) = stat
                .index_worktree_iter(Vec::<gix::bstr::BString>::new())
            {
                for item in res {
                    if let Ok(item) = item {
                        match item {
                            gix::status::index_worktree::Item::Modification { .. } => {
                                status.modified += 1
                            }
                            // Untracked files and others are also returned by this iterator
                            // if configured, but by default it at least gives us modifications.
                            _ => status.modified += 1,
                        }
                    }
                }
            }
        }

        // Ahead/Behind - simplified implementation
        let head = repo.head()?;
        if let Some(Ok(remote_ref)) = head.referent_name().and_then(|name| {
            repo.branch_remote_ref_name(name, gix::remote::Direction::Fetch)
        }) {
            if let Ok(upstream_id) = repo.find_reference(remote_ref.as_ref()).and_then(|r| Ok(r.id())) {
                if let Some(head_id) = head.id() {
                    if let Ok(ahead_walk) = repo.rev_walk([head_id.detach()]).all() {
                        status.ahead = ahead_walk.count();
                    }
                }
            }
        }

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
                if i >= count {
                    break;
                }
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
                    b_dirty
                        .cmp(&a_dirty)
                        .then_with(|| a.branch.cmp(&b.branch))
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
