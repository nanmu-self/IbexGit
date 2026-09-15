use crate::core::engine::{FileStatus, GitEngine};
use crate::core::error::AppError;
use crate::core::watcher::EventKinds;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{mpsc, Mutex, Semaphore};

/// Max parallel read-only git operations across all repositories (PLAN §4.3:
/// 只读操作可并行但限制并发数).
const MAX_CONCURRENT_READS: usize = 8;

/// Per-repository write gate: mutating operations (stage/commit/checkout/…)
/// hold it while running so they execute strictly serially, avoiding
/// `index.lock` conflicts (PLAN §4.3 内部并发).
///
/// [`tokio::sync::Mutex`] is FIFO-fair, so the gate doubles as an ordered
/// operation queue: whoever requests next runs next.
#[derive(Clone, Debug, Default)]
pub struct WriteGate {
    inner: Arc<Mutex<()>>,
}

impl WriteGate {
    pub async fn lock(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.inner.lock().await
    }
}

/// RepoId: FNV-1a hash of the worktree path. Serialized as a string on the
/// wire — u64 exceeds JS `Number.MAX_SAFE_INTEGER` and would truncate.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, specta::Type,
)]
pub struct RepoId(
    #[serde(with = "u64_as_string")]
    #[specta(type = String)]
    pub u64,
);

mod u64_as_string {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(v: &u64, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&v.to_string())
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
        String::deserialize(d)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

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

/// Cached display snapshot for one repository. Per PLAN §4.3 the cache only
/// accelerates display — the `.git` dir and worktree remain the source of
/// truth, and every external event invalidates it.
#[derive(Debug, Clone, specta::Type)]
pub struct StatusSnapshot {
    pub generation: u64,
    /// Duration of the git re-read that produced this snapshot (SLA 埋点).
    pub duration_ms: u64,
    pub status: Vec<FileStatus>,
}

#[derive(Debug, Default)]
struct Session {
    generation: u64,
    status: Option<StatusSnapshot>,
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

/// RepoManager: open/close/list, session cache, per-repo write gates.
#[derive(Clone)]
pub struct RepoManager {
    engine: Arc<dyn GitEngine>,
    repos: Arc<Mutex<HashMap<RepoId, PathBuf>>>,
    gates: Arc<Mutex<HashMap<RepoId, WriteGate>>>,
    sessions: Arc<Mutex<HashMap<RepoId, Session>>>,
    reads: Arc<Semaphore>,
}

impl RepoManager {
    pub fn new(engine: Arc<dyn GitEngine>) -> Self {
        Self {
            engine,
            repos: Arc::new(Mutex::new(HashMap::new())),
            gates: Arc::new(Mutex::new(HashMap::new())),
            sessions: Arc::new(Mutex::new(HashMap::new())),
            reads: Arc::new(Semaphore::new(MAX_CONCURRENT_READS)),
        }
    }

    /// Gate for serializing mutating operations on a repository. Unknown
    /// repositories are rejected so gates can't be created ad hoc.
    pub async fn write_gate(&self, id: RepoId) -> Result<WriteGate, AppError> {
        if self.get_path(id).await.is_none() {
            return Err(AppError::InvalidRepo {
                path: id.0.to_string(),
            });
        }
        Ok(self.gates.lock().await.entry(id).or_default().clone())
    }

    /// Permit for one read-only git operation; caps global parallelism.
    pub async fn read_permit(&self) -> Result<tokio::sync::OwnedSemaphorePermit, AppError> {
        self.reads
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| AppError::internal(e.to_string()))
    }

    /// Shared engine handle (GitEngine trait is the only entry point).
    pub fn engine(&self) -> Arc<dyn GitEngine> {
        self.engine.clone()
    }

    pub async fn open(&self, path: PathBuf) -> Result<RepoId, AppError> {
        // Validate: must be a git worktree. Accept any path *inside* a
        // worktree (dropped files / subfolders) by walking up to the nearest
        // `.git`; the canonical worktree root is what gets opened.
        let mut cur = path.clone();
        loop {
            if cur.join(".git").exists() {
                break;
            }
            match cur.parent() {
                Some(parent) if parent != cur => cur = parent.to_path_buf(),
                _ => {
                    return Err(AppError::InvalidRepo {
                        path: path.display().to_string(),
                    })
                }
            }
        }
        let id = RepoId::new(&cur);
        let mut repos = self.repos.lock().await;
        repos.insert(id, cur);
        Ok(id)
    }

    pub async fn close(&self, id: RepoId) -> Result<(), AppError> {
        let mut repos = self.repos.lock().await;
        repos.remove(&id);
        drop(repos);
        self.sessions.lock().await.remove(&id);
        Ok(())
    }

    /// Invalidate caches after an fs event (external or our own writes), then
    /// re-read status (PLAN §4.3: 失效 → 防抖重读). Returns the new generation,
    /// or `None` for unknown repositories.
    pub async fn invalidate(&self, id: RepoId, kinds: EventKinds) -> Option<u64> {
        let path = {
            let repos = self.repos.lock().await;
            repos.get(&id)?.clone()
        };
        let generation = {
            let mut sessions = self.sessions.lock().await;
            let s = sessions.entry(id).or_default();
            s.generation += 1;
            s.status = None;
            s.generation
        };
        tracing::debug!(
            repo = id.0,
            ?kinds,
            generation,
            sla = "invalidate",
            "cache invalidated; re-reading"
        );

        // 防抖重读：probe status right away so the next frontend read hits cache.
        let started = Instant::now();
        match self.engine.status(&path.display().to_string()).await {
            Ok(status) => {
                let duration_ms = started.elapsed().as_millis() as u64;
                tracing::debug!(
                    repo = id.0,
                    generation,
                    reread_ms = duration_ms,
                    sla = "git_reread",
                    "status re-read after invalidation"
                );
                let mut sessions = self.sessions.lock().await;
                if let Some(s) = sessions.get_mut(&id) {
                    // Only store if no newer invalidation happened meanwhile.
                    if s.generation == generation {
                        s.status = Some(StatusSnapshot {
                            generation,
                            duration_ms,
                            status,
                        });
                    }
                }
            }
            Err(e) => {
                tracing::warn!(
                    repo = id.0,
                    error = %e,
                    "status re-read after invalidation failed; next read retries"
                );
            }
        }
        Some(generation)
    }

    /// Repository status with display-cache fast path (读取即校验版本：
    /// the cache is dropped on every change event, so a hit implies freshness).
    pub async fn status(&self, id: RepoId) -> Result<Vec<FileStatus>, AppError> {
        {
            let sessions = self.sessions.lock().await;
            if let Some(snap) = sessions.get(&id).and_then(|s| s.status.as_ref()) {
                return Ok(snap.status.clone());
            }
        }
        let path = self
            .get_path(id)
            .await
            .ok_or_else(|| AppError::InvalidRepo {
                path: id.0.to_string(),
            })?;
        let started = Instant::now();
        let status = self.engine.status(&path.display().to_string()).await?;
        let duration_ms = started.elapsed().as_millis() as u64;
        tracing::debug!(
            repo = id.0,
            reread_ms = duration_ms,
            sla = "git_reread",
            "cold status read"
        );
        let mut sessions = self.sessions.lock().await;
        let s = sessions.entry(id).or_default();
        s.status = Some(StatusSnapshot {
            generation: s.generation,
            duration_ms,
            status: status.clone(),
        });
        Ok(status)
    }

    pub async fn get_path(&self, id: RepoId) -> Option<PathBuf> {
        let repos = self.repos.lock().await;
        repos.get(&id).cloned()
    }

    pub async fn list(&self) -> Vec<(RepoId, PathBuf)> {
        let repos = self.repos.lock().await;
        repos.iter().map(|(k, v)| (*k, v.clone())).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::engine::{
        BranchInfo, CommitInfo, CommitResult, DiffModel, DiffSource, IndexEntry, PullResult,
        RebaseState, ReflogEntry, RemoteInfo, StashEntry, TagInfo,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Minimal in-memory engine: counts status calls, fails everything else.
    struct MockEngine {
        status_calls: AtomicUsize,
    }

    fn err(feature: &str) -> AppError {
        AppError::not_implemented(feature)
    }

    #[async_trait::async_trait]
    impl GitEngine for MockEngine {
        async fn status(&self, _repo: &str) -> Result<Vec<FileStatus>, AppError> {
            self.status_calls.fetch_add(1, Ordering::SeqCst);
            Ok(vec![FileStatus {
                path: "a.txt".into(),
                status: ".M".into(),
                orig_path: None,
                submodule: false,
                submodule_dirty: false,
                submodule_commit_changed: false,
                eol_only: false,
                staged: false,
                unstaged: true,
                untracked: false,
                skipped: false,
                conflict: false,
            }])
        }
        async fn stage(&self, _: &str, _: &[String]) -> Result<(), AppError> {
            Err(err("stage"))
        }
        async fn unstage(&self, _: &str, _: &[String]) -> Result<(), AppError> {
            Err(err("unstage"))
        }
        async fn restore_worktree(&self, _: &str, _: &[String]) -> Result<(), AppError> {
            Err(err("restore_worktree"))
        }
        async fn restore_to_head(&self, _: &str, _: &[String]) -> Result<(), AppError> {
            Err(err("restore_to_head"))
        }
        async fn delete_untracked(&self, _: &str, _: &[String]) -> Result<(), AppError> {
            Err(err("delete_untracked"))
        }
        async fn head_message(&self, _: &str) -> Result<Option<String>, AppError> {
            Err(err("head_message"))
        }
        async fn ignore_paths(&self, _: &str, _: &[String]) -> Result<(), AppError> {
            Err(err("ignore_paths"))
        }
        async fn ls_index(&self, _: &str, _: &[String]) -> Result<Vec<IndexEntry>, AppError> {
            Err(err("ls_index"))
        }
        async fn update_index_info(&self, _: &str, _: &str) -> Result<(), AppError> {
            Err(err("update_index_info"))
        }
        async fn remove_index_entries(&self, _: &str, _: &[String]) -> Result<(), AppError> {
            Err(err("remove_index_entries"))
        }
        async fn commit(
            &self,
            _: &str,
            _: &str,
            _: bool,
            _: bool,
        ) -> Result<CommitResult, AppError> {
            Err(err("commit"))
        }
        async fn diff(
            &self,
            _: &str,
            _: DiffSource,
            _: Option<(&str, &str)>,
            _: &[String],
        ) -> Result<DiffModel, AppError> {
            Err(err("diff"))
        }
        async fn log(
            &self,
            _: &str,
            _: u32,
            _: u32,
            _: &[String],
        ) -> Result<Vec<CommitInfo>, AppError> {
            Err(err("log"))
        }
        async fn list_branches(&self, _: &str) -> Result<Vec<BranchInfo>, AppError> {
            Err(err("branches"))
        }
        async fn create_branch(&self, _: &str, _: &str, _: Option<&str>) -> Result<(), AppError> {
            Err(err("create_branch"))
        }
        async fn delete_branch(&self, _: &str, _: &str, _: bool) -> Result<(), AppError> {
            Err(err("delete_branch"))
        }
        async fn rename_branch(&self, _: &str, _: &str, _: &str) -> Result<(), AppError> {
            Err(err("rename_branch"))
        }
        async fn checkout_branch(&self, _: &str, _: &str) -> Result<(), AppError> {
            Err(err("checkout_branch"))
        }
        async fn list_tags(&self, _: &str) -> Result<Vec<TagInfo>, AppError> {
            Err(err("tags"))
        }
        async fn create_tag(
            &self,
            _: &str,
            _: &str,
            _: Option<&str>,
            _: &str,
        ) -> Result<(), AppError> {
            Err(err("create_tag"))
        }
        async fn delete_tag(&self, _: &str, _: &str) -> Result<(), AppError> {
            Err(err("delete_tag"))
        }
        async fn list_stash(&self, _: &str) -> Result<Vec<StashEntry>, AppError> {
            Err(err("stash"))
        }
        async fn stash_push(&self, _: &str, _: Option<&str>) -> Result<usize, AppError> {
            Err(err("stash_push"))
        }
        async fn stash_pop(&self, _: &str, _: usize) -> Result<(), AppError> {
            Err(err("stash_pop"))
        }
        async fn stash_drop(&self, _: &str, _: usize) -> Result<(), AppError> {
            Err(err("stash_drop"))
        }
        async fn list_remotes(&self, _: &str) -> Result<Vec<RemoteInfo>, AppError> {
            Err(err("remotes"))
        }
        async fn fetch(&self, _: &str, _: Option<&str>) -> Result<(), AppError> {
            Err(err("fetch"))
        }
        async fn reflog(&self, _: &str, _: Option<&str>) -> Result<Vec<ReflogEntry>, AppError> {
            Err(err("reflog"))
        }
        async fn reset(&self, _: &str, _: &str, _: &str) -> Result<(), AppError> {
            Err(err("reset"))
        }
        async fn apply(&self, _: &str, _: &str, _: bool, _: bool) -> Result<(), AppError> {
            Err(err("apply"))
        }
        async fn push(&self, _: &str, _: &str, _: &str, _: bool, _: bool) -> Result<(), AppError> {
            Err(err("push"))
        }
        async fn pull(
            &self,
            _: &str,
            _: Option<&str>,
            _: Option<&str>,
            _: Option<&str>,
        ) -> Result<PullResult, AppError> {
            Err(err("pull"))
        }
        async fn rebase(&self, _: &str, _: &str, _: &[&str]) -> Result<RebaseState, AppError> {
            Err(err("rebase"))
        }
        async fn merge(
            &self,
            _: &str,
            _: &str,
            _: Option<&str>,
        ) -> Result<crate::core::engine::MergeResult, AppError> {
            Err(err("merge"))
        }
        async fn blame(&self, _: &str, _: &str) -> Result<Vec<ReflogEntry>, AppError> {
            Err(err("blame"))
        }
    }

    fn temp_repo() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ibexgit-repomgr-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        dir
    }

    #[tokio::test]
    async fn status_caches_until_invalidation() {
        let engine = Arc::new(MockEngine {
            status_calls: AtomicUsize::new(0),
        });
        let mgr = RepoManager::new(engine.clone() as Arc<dyn GitEngine>);
        let dir = temp_repo();
        let id = mgr.open(dir.clone()).await.unwrap();

        // Two reads → one engine call (cache hit on the second).
        let s1 = mgr.status(id).await.unwrap();
        let s2 = mgr.status(id).await.unwrap();
        assert_eq!(s1.len(), 1);
        assert_eq!(s1, s2);
        assert_eq!(engine.status_calls.load(Ordering::SeqCst), 1);

        // Invalidation re-reads immediately (防抖重读) → call 2.
        let gen = mgr.invalidate(id, EventKinds::INDEX).await.unwrap();
        assert_eq!(gen, 1);
        assert_eq!(engine.status_calls.load(Ordering::SeqCst), 2);

        // Subsequent read hits the fresh cache → still 2.
        let s3 = mgr.status(id).await.unwrap();
        assert_eq!(s3, s1);
        assert_eq!(engine.status_calls.load(Ordering::SeqCst), 2);

        // Generation monotonically increases per event.
        let gen2 = mgr.invalidate(id, EventKinds::WORKTREE).await.unwrap();
        assert_eq!(gen2, 2);
        assert_eq!(engine.status_calls.load(Ordering::SeqCst), 3);

        mgr.close(id).await.unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn invalidate_unknown_repo_returns_none() {
        let engine = Arc::new(MockEngine {
            status_calls: AtomicUsize::new(0),
        });
        let mgr = RepoManager::new(engine.clone() as Arc<dyn GitEngine>);
        assert!(mgr
            .invalidate(RepoId(999), EventKinds::INDEX)
            .await
            .is_none());
    }

    #[tokio::test]
    async fn write_gate_serializes_and_rejects_unknown_repo() {
        let engine = Arc::new(MockEngine {
            status_calls: AtomicUsize::new(0),
        });
        let mgr = RepoManager::new(engine as Arc<dyn GitEngine>);
        assert!(mgr.write_gate(RepoId(42)).await.is_err());

        let dir = temp_repo();
        let id = mgr.open(dir.clone()).await.unwrap();
        let gate = mgr.write_gate(id).await.unwrap();

        use std::sync::atomic::AtomicUsize;
        let inside = Arc::new(AtomicUsize::new(0));

        // Hold the gate; a second contender must wait.
        let g1 = gate.lock().await;
        let contender = {
            let gate = gate.clone();
            let inside = inside.clone();
            tokio::spawn(async move {
                let _g = gate.lock().await;
                inside.fetch_add(1, Ordering::SeqCst);
            })
        };
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(
            inside.load(Ordering::SeqCst),
            0,
            "contender must not enter while gate is held"
        );
        drop(g1);
        contender.await.unwrap();
        assert_eq!(inside.load(Ordering::SeqCst), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// PLAN §4.3 P1 acceptance: hammering stage concurrently must never
    /// surface index.lock errors — the gate serializes the writes.
    #[tokio::test(flavor = "multi_thread")]
    async fn concurrent_stages_never_hit_index_lock() {
        use crate::core::engine::CliEngine;
        use crate::core::runner::GitProcessRunner;
        use std::process::Command;

        let dir = std::env::temp_dir().join(format!(
            "ibexgit-concurrent-stage-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(Command::new("git")
            .args(["init", "-q"])
            .current_dir(&dir)
            .status()
            .expect("git available")
            .success());
        assert!(Command::new("git")
            .args(["-C", dir.to_str().unwrap(), "config", "user.email", "t@t"])
            .status()
            .unwrap()
            .success());
        assert!(Command::new("git")
            .args(["-C", dir.to_str().unwrap(), "config", "user.name", "t"])
            .status()
            .unwrap()
            .success());

        let engine: Arc<dyn GitEngine> = Arc::new(CliEngine::new(GitProcessRunner::new(60), "git"));
        let mgr = RepoManager::new(engine);
        let id = mgr.open(dir.clone()).await.unwrap();
        let gate = mgr.write_gate(id).await.unwrap();

        let n = 8;
        let mut handles = Vec::new();
        for i in 0..n {
            let gate = gate.clone();
            let mgr = mgr.clone();
            let path = dir.clone();
            handles.push(tokio::spawn(async move {
                // Each task stages a distinct file, concurrently.
                let file = path.join(format!("f{}.txt", i));
                std::fs::write(&file, format!("content {}", i)).unwrap();
                let _guard = gate.lock().await;
                mgr.engine()
                    .stage(&path.display().to_string(), &[format!("f{}.txt", i)])
                    .await
            }));
        }
        for h in handles {
            let res = h.await.unwrap();
            assert!(res.is_ok(), "concurrent stage failed: {:?}", res.err());
        }
        mgr.close(id).await.unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn read_permit_caps_parallelism() {
        let engine = Arc::new(MockEngine {
            status_calls: AtomicUsize::new(0),
        });
        let mgr = RepoManager::new(engine as Arc<dyn GitEngine>);
        let mut permits = Vec::new();
        for _ in 0..MAX_CONCURRENT_READS {
            permits.push(mgr.read_permit().await.unwrap());
        }
        // All permits held → the next acquire must pend (not error, not pass).
        let mgr2 = std::sync::Arc::new(mgr);
        let waiter = {
            let mgr = mgr2.clone();
            tokio::spawn(async move { mgr.read_permit().await })
        };
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(!waiter.is_finished(), "9th permit must wait");
        drop(permits);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(500), waiter)
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn close_drops_session_cache() {
        let engine = Arc::new(MockEngine {
            status_calls: AtomicUsize::new(0),
        });
        let mgr = RepoManager::new(engine.clone() as Arc<dyn GitEngine>);
        let dir = temp_repo();
        let id = mgr.open(dir.clone()).await.unwrap();
        let _ = mgr.status(id).await.unwrap();
        mgr.close(id).await.unwrap();
        // Re-open same path: fresh session → cold read again.
        mgr.open(dir.clone()).await.unwrap();
        let _ = mgr.status(id).await.unwrap();
        assert_eq!(engine.status_calls.load(Ordering::SeqCst), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
