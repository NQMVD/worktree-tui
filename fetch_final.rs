fn fetch_all_worktrees(repo_root: &PathBuf, _current_path: &PathBuf) -> Result<Vec<Worktree>> {
    let _span = info_span!("fetch_all_worktrees").entered();
    info!(repo_root = %repo_root.display(), "Starting fetch_all_worktrees");
    
    let repo = match gix::open(repo_root) {
        Ok(r) => {
            info!(path = ?r.path(), "Successfully opened repository at repo_root");
            r
        }
        Err(e) => {
            info!(error = ?e, "FAILED to open repository at repo_root");
            return Err(e.into());
        }
    };

    let worktree_proxies = match repo.worktrees() {
        Ok(wp) => {
            info!(count = wp.len(), "Worktree proxies retrieved");
            wp
        }
        Err(e) => {
            info!(error = ?e, "FAILED to retrieve worktree proxies");
            return Err(e.into());
        }
    };

    let mut worktrees = Vec::new();

    // Helper to create Worktree struct
    let get_wt_info = |proxy: Option<gix::worktree::Proxy<'_>>, r: &gix::Repository| -> Result<Worktree> {
        let (path, branch, commit, is_main, is_locked, lock_reason) = match proxy {
            Some(p) => {
                let path = p.base()?.to_path_buf();
                let is_locked = p.lock_reason().is_some();
                let lock_reason = p.lock_reason().map(|s| s.to_string());
                
                info!(?path, ?is_locked, "Processing linked worktree proxy");
                
                let wt_repo = p.into_repo().context("Failed to open worktree repo from proxy")?;
                let head = wt_repo.head().context("Failed to get HEAD for worktree")?;
                let branch = head.referent_name().map(|n| n.shorten().to_string());
                let commit = head.id().map(|id| id.to_string()).unwrap_or_default();
                (path, branch, commit, false, is_locked, lock_reason)
            }
            None => {
                let path = r.work_dir().map(|p| p.to_path_buf()).unwrap_or_else(|| r.common_dir().to_path_buf());
                info!(?path, "Processing main worktree");
                
                let head = r.head().context("Failed to get HEAD for main repo")?;
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
    match get_wt_info(None, &repo) {
        Ok(wt) => {
            info!(path = %wt.path.display(), is_main = wt.is_main, "Added main worktree to list");
            worktrees.push(wt);
        }
        Err(e) => {
            info!(error = ?e, "Failed to get main worktree info");
        }
    }

    // Add linked worktrees
    for proxy in worktree_proxies {
        match get_wt_info(Some(proxy), &repo) {
            Ok(wt) => {
                info!(path = %wt.path.display(), is_main = wt.is_main, "Added linked worktree to list");
                worktrees.push(wt);
            }
            Err(e) => {
                info!(error = ?e, "Failed to get linked worktree info");
            }
        }
    }

    info!(count = worktrees.len(), "Worktree base list completed, starting detail fetch in parallel");

    // Fetch additional status for each worktree IN PARALLEL
    std::thread::scope(|s| {
        let mut task_handles = Vec::new();
        
        for (i, wt) in worktrees.iter().enumerate() {
            if wt.is_bare || wt.is_prunable { 
                info!(idx = i, path = %wt.path.display(), is_bare = wt.is_bare, is_prunable = wt.is_prunable, "Skipping details fetch");
                continue; 
            }
            let path = wt.path.clone();
            
            task_handles.push(s.spawn(move || {
                let _span = info_span!("fetch_wt_details", wt_idx = i, path = %path.display()).entered();
                match gix::open(&path) {
                    Ok(repo) => {
                        let status = App::get_gix_status(&repo).unwrap_or_else(|e| {
                            info!(error = ?e, "Status fetch failed for worktree");
                            WorktreeStatus::default()
                        });
                        let commit_info = App::get_gix_commit_info(&repo).unwrap_or_else(|e| {
                            info!(error = ?e, "Commit info fetch failed for worktree");
                            (String::new(), None)
                        });
                        let recent_commits = App::get_gix_recent_commits(&repo, 10).unwrap_or_else(|e| {
                            info!(error = ?e, "Recent commits fetch failed for worktree");
                            Vec::new()
                        });
                        (i, status, commit_info, recent_commits)
                    }
                    Err(e) => {
                        info!(error = ?e, "Failed to open repo at worktree path for details");
                        (i, WorktreeStatus::default(), (String::new(), None), Vec::new())
                    }
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
    
    info!(count = worktrees.len(), "Finished all worktree fetching");
    Ok(worktrees)
}
