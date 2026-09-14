use crate::core::engine::GitEngine;
use crate::core::error::AppError;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct RepoId(pub u64);

impl RepoId {
    pub fn new(path: &Path) -> Self {
        let mut h: u64 = 1469598103934665603;
        for b in path.as_os_str().as_encoded_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(1099511628211);
        }
        RepoId(h)
    }
}

/// A queued operation (serialized per-repo).
#[derive(Debug)]
pub struct Operation {
    pub id: u64,
    pub repo_id: RepoId,
    pub kind: String,
    pub payload: Vec<u8>,
}

/// Handle to a queued operation's response channel.
#[derive(Debug)]
pub struct OpHandle {
    pub id: u64,
    pub ack: tokio::sync::oneshot::Sender<Result<Vec<u8>, AppError>>,
}

/// Per-repo queue ensuring serial execution to avoid index.lock conflicts.
#[derive(Clone)]
#[allow(dead_code)]
pub struct RepoQueue {
    repo_id: RepoId,
    sender: mpsc::UnboundedSender<Operation>,
}

impl RepoQueue {
    pub fn new(repo_id: RepoId) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<Operation>();
        let repo_id_clone = repo_id;

        tokio::spawn(async move {
            while let Some(_op) = rx.recv().await {
                // P1 placeholder: dispatch will be implemented with engine.
                let _ = _op;
            }
        });

        Self {
            repo_id: repo_id_clone,
            sender: tx,
        }
    }

    pub fn submit(&self, op: Operation) -> OpHandle {
        let (ack, _rx) = tokio::sync::oneshot::channel::<Result<Vec<u8>, AppError>>();
        let _ = self.sender.send(op);
        OpHandle { id: 0, ack }
    }
}

/// RepoManager: open/close/list, session cache, per-repo queues.
#[allow(dead_code)]
pub struct RepoManager {
    engine: Arc<dyn GitEngine>,
    repos: Arc<Mutex<HashMap<RepoId, PathBuf>>>,
    queues: Arc<Mutex<HashMap<RepoId, RepoQueue>>>,
}

impl RepoManager {
    pub fn new(engine: Arc<dyn GitEngine>) -> Self {
        Self {
            engine,
            repos: Arc::new(Mutex::new(HashMap::new())),
            queues: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Shared engine handle (GitEngine trait is the only entry point).
    pub fn engine(&self) -> Arc<dyn GitEngine> {
        self.engine.clone()
    }

    pub async fn open(&self, path: PathBuf) -> Result<RepoId, AppError> {
        // Validate: must be a git worktree (.git dir/file present).
        let dot_git = path.join(".git");
        if !dot_git.exists() {
            return Err(AppError::InvalidRepo {
                path: path.display().to_string(),
            });
        }
        let id = RepoId::new(&path);
        let mut repos = self.repos.lock().await;
        repos.insert(id, path);
        Ok(id)
    }

    pub async fn close(&self, id: RepoId) -> Result<(), AppError> {
        let mut repos = self.repos.lock().await;
        repos.remove(&id);
        Ok(())
    }

    pub async fn get_path(&self, id: RepoId) -> Option<PathBuf> {
        let repos = self.repos.lock().await;
        repos.get(&id).cloned()
    }

    pub async fn list(&self) -> Vec<(RepoId, PathBuf)> {
        let repos = self.repos.lock().await;
        repos.iter().map(|(k, v)| (*k, v.clone())).collect()
    }

    pub async fn queue(&self, id: RepoId) -> RepoQueue {
        let mut queues = self.queues.lock().await;
        queues
            .entry(id)
            .or_insert_with(|| RepoQueue::new(id))
            .clone()
    }
}
