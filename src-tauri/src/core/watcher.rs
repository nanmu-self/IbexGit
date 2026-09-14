use crate::core::error::AppError;
use notify::{Config, RecursiveMode, Watcher};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

#[derive(Debug, Clone)]
pub struct RepoEvent {
    pub repo_id: String,
    pub kind: EventKind,
    pub paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    HeadChanged,
    IndexChanged,
    RefsChanged,
    MergeHeadChanged,
    WorktreeChanged,
    ConfigChanged,
}

/// Watches a single repository for changes.
#[allow(dead_code)]
pub struct RepoWatcher {
    repo_id: String,
    _git_dir: PathBuf,
    _worktree: PathBuf,
    _watcher: Option<notify::RecommendedWatcher>,
}

impl RepoWatcher {
    pub fn new(
        repo_id: impl Into<String>,
        git_dir: PathBuf,
        worktree: PathBuf,
        tx: mpsc::UnboundedSender<RepoEvent>,
    ) -> Result<Self, AppError> {
        let repo_id = repo_id.into();
        let repo_id_clone = repo_id.clone();
        let mut watcher = notify::RecommendedWatcher::new(
            move |res: Result<notify::Event, notify::Error>| {
                if let Ok(event) = res {
                    let kind = if event.paths.iter().any(|p| p.ends_with("HEAD")) {
                        EventKind::HeadChanged
                    } else if event.paths.iter().any(|p| p.ends_with("index")) {
                        EventKind::IndexChanged
                    } else if event.paths.iter().any(|p| p.starts_with("refs")) {
                        EventKind::RefsChanged
                    } else if event.paths.iter().any(|p| p.ends_with("MERGE_HEAD")) {
                        EventKind::MergeHeadChanged
                    } else if event.paths.iter().any(|p| p.starts_with(".git")) {
                        EventKind::ConfigChanged
                    } else {
                        EventKind::WorktreeChanged
                    };
                    let _ = tx.send(RepoEvent {
                        repo_id: repo_id_clone.clone(),
                        kind,
                        paths: event.paths,
                    });
                }
            },
            Config::default(),
        )
        .map_err(|e| AppError::io_with_detail("watcher", e.to_string()))?;

        let _ = watcher.watch(&git_dir, RecursiveMode::NonRecursive);
        let _ = watcher.watch(&worktree, RecursiveMode::Recursive);

        Ok(Self {
            repo_id,
            _git_dir: git_dir,
            _worktree: worktree,
            _watcher: Some(watcher),
        })
    }
}

/// Central watcher hub; dispatches events to per-repo watchers and emits repo://changed after debounce.
#[allow(dead_code)]
pub struct WatcherHub {
    tx: mpsc::UnboundedSender<RepoEvent>,
    rx: Arc<RwLock<mpsc::UnboundedReceiver<RepoEvent>>>,
    _watchers: Arc<RwLock<HashMap<String, RepoWatcher>>>,
}

impl WatcherHub {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::unbounded_channel::<RepoEvent>();
        Self {
            tx,
            rx: Arc::new(RwLock::new(rx)),
            _watchers: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn add_repo(
        &self,
        repo_id: impl Into<String>,
        git_dir: PathBuf,
        worktree: PathBuf,
    ) -> Result<(), AppError> {
        let repo_id = repo_id.into();
        let watcher = RepoWatcher::new(repo_id.clone(), git_dir, worktree, self.tx.clone())?;
        self._watchers.write().await.insert(repo_id, watcher);
        Ok(())
    }

    pub async fn remove_repo(&self, repo_id: &str) {
        self._watchers.write().await.remove(repo_id);
    }
}

use std::collections::HashMap;

impl Default for WatcherHub {
    fn default() -> Self {
        Self::new()
    }
}
