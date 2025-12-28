//! Cache module for persisting worktree data to disk
//! Enables instant startup by loading cached data while refreshing in background

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// How long cached data is considered "fresh" (no background refresh needed)
const CACHE_TTL_SECS: u64 = 10;

/// Serializable worktree status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedWorktreeStatus {
    pub modified: usize,
    pub staged: usize,
    pub untracked: usize,
    pub ahead: usize,
    pub behind: usize,
}

/// Serializable commit info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedCommitInfo {
    pub hash: String,
    pub message: String,
    pub time_ago: String,
}

/// Serializable worktree data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedWorktree {
    pub path: PathBuf,
    pub branch: Option<String>,
    pub commit: String,
    pub commit_short: String,
    pub commit_message: String,
    pub commit_time: Option<i64>,
    pub is_main: bool,
    pub is_current: bool,
    pub is_bare: bool,
    pub is_detached: bool,
    pub is_locked: bool,
    pub lock_reason: Option<String>,
    pub is_prunable: bool,
    pub status: CachedWorktreeStatus,
    pub recent_commits: Vec<CachedCommitInfo>,
}

/// The full cache structure with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorktreeCache {
    /// Unix timestamp when cache was written
    pub timestamp: u64,
    /// The repo root this cache is for
    pub repo_root: PathBuf,
    /// Cached worktree data
    pub worktrees: Vec<CachedWorktree>,
}

impl WorktreeCache {
    /// Check if the cache is still fresh (within TTL)
    pub fn is_fresh(&self) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        now.saturating_sub(self.timestamp) < CACHE_TTL_SECS
    }

    /// Get age of cache in seconds
    #[allow(dead_code)]
    pub fn age_secs(&self) -> u64 {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        now.saturating_sub(self.timestamp)
    }
}

/// Get the cache directory path (~/.cache/wtt/)
fn cache_dir() -> Option<PathBuf> {
    dirs::cache_dir().map(|d| d.join("wtt"))
}

/// Get the cache file path for a specific repo
fn cache_file_path(repo_root: &PathBuf) -> Option<PathBuf> {
    // Use a hash of the repo path to create unique cache files per repo
    let hash = {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        repo_root.hash(&mut hasher);
        format!("{:x}", hasher.finish())
    };
    cache_dir().map(|d| d.join(format!("{}.json", hash)))
}

/// Load cache from disk for a specific repo
pub fn load_cache(repo_root: &PathBuf) -> Option<WorktreeCache> {
    let path = cache_file_path(repo_root)?;

    if !path.exists() {
        return None;
    }

    let content = fs::read_to_string(&path).ok()?;
    let cache: WorktreeCache = serde_json::from_str(&content).ok()?;

    // Verify this cache is for the right repo
    if cache.repo_root != *repo_root {
        return None;
    }

    Some(cache)
}

/// Save cache to disk
pub fn save_cache(cache: &WorktreeCache) -> Result<(), std::io::Error> {
    let dir = cache_dir().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Could not determine cache directory",
        )
    })?;

    // Create cache directory if it doesn't exist
    fs::create_dir_all(&dir)?;

    let path = cache_file_path(&cache.repo_root).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Could not determine cache file path",
        )
    })?;

    let content = serde_json::to_string_pretty(cache)?;
    fs::write(&path, content)?;

    Ok(())
}

/// Create a new cache with current timestamp
pub fn create_cache(repo_root: PathBuf, worktrees: Vec<CachedWorktree>) -> WorktreeCache {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    WorktreeCache {
        timestamp,
        repo_root,
        worktrees,
    }
}
