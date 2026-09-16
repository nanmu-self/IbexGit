use crate::core::engine::{DiffModel, FileStatus, GitEngine, GraphFilter, GraphPage};
use crate::core::error::AppError;
use crate::core::graph::{GraphRow, LayoutState};
use crate::core::watcher::EventKinds;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{mpsc, Mutex, Semaphore};

/// Max parallel read-only git operations across all repositories (PLAN §4.3:
/// 只读操作可并行但限制并发数).
const MAX_CONCURRENT_READS: usize = 8;

/// Max cached DiffModels per repository (line-level operations reference
/// them by id; oldest ids are evicted first).
const MAX_CACHED_DIFFS: u32 = 8;

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

/// Per-repository page size of the commit graph (PLAN P5: 分批 500/批).
pub const GRAPH_BATCH: u32 = 500;

#[derive(Debug, Default)]
struct Session {
    generation: u64,
    status: Option<StatusSnapshot>,
    /// Monotonic DiffModel id source (never reused within a session).
    next_diff_id: u32,
    /// Cached DiffModels for line-level operations (PLAN P4 同源保证):
    /// the frontend sends {model_id, path, selections} and Rust builds the
    /// patch from the same model it served. Cleared on every invalidation.
    diffs: HashMap<u32, DiffModel>,
    /// Commit graph cache (PLAN P5): cache key = refs generation (bumped by
    /// every watcher invalidation) + serialized filter. v1 semantics: any
    /// invalidation discards the graph and the next request recomputes from
    /// scratch; loaded batches accumulate via `state` (lane continuity).
    graph: Option<CachedGraph>,
}

#[derive(Debug)]
struct CachedGraph {
    generation: u64,
    filter_key: String,
    rows: Vec<GraphRow>,
    state: LayoutState,
    /// `git log` exhausted (no further batches).
    complete: bool,
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
            // DiffModels describe a point-in-time state; after any change
            // they are suspect and must not feed line-level patches.
            s.diffs.clear();
            // Graph cache key = refs generation → any event rebuilds (v1
            // 失效即全量重算).
            s.graph = None;
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

    /// Commit-graph page (PLAN P5 四层管线的取数+缓存+布局编排).
    ///
    /// `more = false`: ensure the graph is fresh, return the first page.
    /// `more = true`: fetch the next batch and append to the cached lanes;
    /// if the cache was invalidated meanwhile the graph rebuilds and the
    /// returned `start` is 0 (the frontend replaces its list).
    ///
    /// The sessions lock is held across the git fetch (v1 simplicity): the
    /// alternative — optimistic append with race re-validation — buys a
    /// few hundred ms of watcher parallelism at the cost of intricate
    /// retry logic; graph requests are infrequent (scroll-driven).
    pub async fn graph_page(
        &self,
        id: RepoId,
        filter: &GraphFilter,
        more: bool,
        batch: u32,
    ) -> Result<GraphPage, AppError> {
        let path = self
            .get_path(id)
            .await
            .ok_or_else(|| AppError::InvalidRepo {
                path: id.0.to_string(),
            })?;
        let filter_key = serde_json::to_string(filter)?;
        let mut sessions = self.sessions.lock().await;
        let s = sessions.entry(id).or_default();

        let rebuild = match s.graph.as_ref() {
            Some(g) => g.generation != s.generation || g.filter_key != filter_key,
            None => true,
        };

        if rebuild {
            let commits = self
                .engine
                .graph(&path.display().to_string(), 0, batch, filter)
                .await?;
            let mut state = LayoutState::new();
            let mut rows = Vec::new();
            state.layout(&commits, &mut rows);
            let complete = (commits.len() as u32) < batch;
            let width = state.width as u32;
            s.graph = Some(CachedGraph {
                generation: s.generation,
                filter_key,
                rows,
                state,
                complete,
            });
            let g = s.graph.as_ref().expect("just inserted");
            return Ok(GraphPage {
                rows: g.rows.clone(),
                start: 0,
                complete,
                width,
            });
        }

        let g = s.graph.as_mut().expect("cached graph");
        // First page of a valid cache: serve the loaded rows (≤ batch).
        // Must run BEFORE the complete-graph check — the empty delta below
        // is page-append semantics meant for `more` requests only; serving
        // it to a first-page request wiped the list on every refresh
        // (history "disappeared" after F5 / a branch switch).
        if !more {
            let end = g.rows.len().min(batch as usize);
            return Ok(GraphPage {
                rows: g.rows[..end].to_vec(),
                start: 0,
                complete: g.complete,
                width: g.state.width as u32,
            });
        }
        if g.complete {
            // Nothing more to load; serve an empty delta.
            return Ok(GraphPage {
                rows: Vec::new(),
                start: g.rows.len() as u32,
                complete: true,
                width: g.state.width as u32,
            });
        }

        let skip = g.rows.len() as u32;
        let commits = self
            .engine
            .graph(&path.display().to_string(), skip, batch, filter)
            .await?;
        let complete = (commits.len() as u32) < batch;
        let start = g.rows.len() as u32;
        let fresh: Vec<GraphRow> = {
            let mut out = Vec::new();
            g.state.layout(&commits, &mut out);
            out
        };
        let width = g.state.width as u32;
        g.rows.extend(fresh.iter().cloned());
        g.complete = complete;
        Ok(GraphPage {
            rows: fresh,
            start,
            complete,
            width,
        })
    }

    pub async fn get_path(&self, id: RepoId) -> Option<PathBuf> {
        let repos = self.repos.lock().await;
        repos.get(&id).cloned()
    }

    /// Store a DiffModel for later line-level operations; assigns and
    /// returns its id (mutates the model in place).
    pub async fn cache_diff(&self, id: RepoId, model: &mut DiffModel) -> Result<(), AppError> {
        let mut sessions = self.sessions.lock().await;
        let s = sessions.entry(id).or_default();
        s.next_diff_id += 1;
        model.id = s.next_diff_id;
        // Evict the oldest ids beyond the cap (ids are monotonic).
        let floor = s.next_diff_id.saturating_sub(MAX_CACHED_DIFFS);
        s.diffs.retain(|k, _| *k > floor);
        s.diffs.insert(model.id, model.clone());
        Ok(())
    }

    /// Fetch a cached DiffModel by id. Errors with [`AppError::DiffModelExpired`]
    /// when the model was invalidated or evicted — the frontend must re-fetch.
    pub async fn get_diff(&self, id: RepoId, model_id: u32) -> Result<DiffModel, AppError> {
        let sessions = self.sessions.lock().await;
        sessions
            .get(&id)
            .and_then(|s| s.diffs.get(&model_id))
            .cloned()
            .ok_or(AppError::DiffModelExpired)
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
        BackupRef, BlameResult, BranchInfo, CloneOptions, CommitInfo, CommitResult, DiffModel,
        DiffSource, FileCommit, IndexEntry, PullResult, RebaseState, ReflogEntry, RemoteInfo,
        StashEntry, TagInfo,
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
        async fn clone_repo(
            &self,
            _opts: &CloneOptions,
            _cancel: Option<&crate::core::runner::CancelToken>,
            _on_line: Option<Arc<dyn Fn(String) + Send + Sync>>,
        ) -> Result<(), AppError> {
            Err(AppError::not_implemented("clone"))
        }

        async fn init_repo(&self, _path: &str) -> Result<(), AppError> {
            Err(AppError::not_implemented("init"))
        }

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
            _: crate::core::engine::DiffOptions,
        ) -> Result<DiffModel, AppError> {
            Err(err("diff"))
        }
        async fn file_content(
            &self,
            _: &str,
            _: &str,
            _: Option<&str>,
        ) -> Result<crate::core::engine::FileContent, AppError> {
            Err(err("file_content"))
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
        async fn graph(
            &self,
            _: &str,
            _: u32,
            _: u32,
            _: &crate::core::engine::GraphFilter,
        ) -> Result<Vec<CommitInfo>, AppError> {
            Err(err("graph"))
        }
        async fn commit_detail(
            &self,
            _: &str,
            _: &str,
        ) -> Result<crate::core::engine::CommitDetail, AppError> {
            Err(err("commit_detail"))
        }
        async fn cherry_pick(&self, _: &str, _: &[String]) -> Result<(), AppError> {
            Err(err("cherry_pick"))
        }
        async fn revert(&self, _: &str, _: &[String]) -> Result<(), AppError> {
            Err(err("revert"))
        }
        async fn restore_from(&self, _: &str, _: &str, _: &[String]) -> Result<(), AppError> {
            Err(err("restore_from"))
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
        async fn set_branch_upstream(
            &self,
            _: &str,
            _: &str,
            _: Option<&str>,
        ) -> Result<(), AppError> {
            Err(err("set_branch_upstream"))
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
        async fn stash_apply(&self, _: &str, _: usize) -> Result<(), AppError> {
            Err(err("stash_apply"))
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
        async fn add_remote(&self, _: &str, _: &str, _: &str) -> Result<(), AppError> {
            Err(err("add_remote"))
        }
        async fn remove_remote(&self, _: &str, _: &str) -> Result<(), AppError> {
            Err(err("remove_remote"))
        }
        async fn set_remote_url(&self, _: &str, _: &str, _: &str, _: bool) -> Result<(), AppError> {
            Err(err("set_remote_url"))
        }
        async fn prune_remote(&self, _: &str, _: &str) -> Result<(), AppError> {
            Err(err("prune_remote"))
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
        async fn clean_list(&self, _: &str) -> Result<Vec<String>, AppError> {
            Err(err("clean_list"))
        }
        async fn clean(&self, _: &str, _: &[String]) -> Result<(), AppError> {
            Err(err("clean"))
        }
        async fn merge_base(&self, _: &str, _: &str, _: &str) -> Result<Option<String>, AppError> {
            Err(err("merge_base"))
        }
        async fn range_count(&self, _: &str, _: &str, _: &str) -> Result<(u32, u32), AppError> {
            Err(err("range_count"))
        }
        async fn rev_list(
            &self,
            _: &str,
            _: &str,
            _: u32,
            _: u32,
        ) -> Result<Vec<CommitInfo>, AppError> {
            Err(err("rev_list"))
        }
        async fn update_ref(&self, _: &str, _: &str, _: &str) -> Result<(), AppError> {
            Err(err("update_ref"))
        }
        async fn list_backup_refs(&self, _: &str) -> Result<Vec<BackupRef>, AppError> {
            Err(err("list_backup_refs"))
        }
        async fn delete_backup_refs(&self, _: &str, _: &[String]) -> Result<(), AppError> {
            Err(err("delete_backup_refs"))
        }
        async fn apply(&self, _: &str, _: &str, _: bool, _: bool) -> Result<(), AppError> {
            Err(err("apply"))
        }
        async fn push(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: bool,
            _: bool,
            _: bool,
        ) -> Result<(), AppError> {
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
            _: bool,
        ) -> Result<crate::core::engine::MergeResult, AppError> {
            Err(err("merge"))
        }
        async fn file_history(
            &self,
            _: &str,
            _: &str,
            _: u32,
            _: Option<&str>,
        ) -> Result<Vec<FileCommit>, AppError> {
            Err(err("file_history"))
        }
        async fn blame(&self, _: &str, _: &str) -> Result<BlameResult, AppError> {
            Err(err("blame"))
        }

        // =====================
        // P8 (MockEngine never serves conflict flows)
        // =====================
        async fn conflict_list(
            &self,
            _: &str,
        ) -> Result<Vec<crate::core::engine::conflict::ConflictSummary>, AppError> {
            Err(err("conflict_list"))
        }
        async fn conflict_model(
            &self,
            _: &str,
            _: &str,
        ) -> Result<crate::core::engine::conflict::ConflictModel, AppError> {
            Err(err("conflict_model"))
        }
        async fn resolve_conflict_text(&self, _: &str, _: &str, _: &str) -> Result<(), AppError> {
            Err(err("resolve_conflict_text"))
        }
        async fn resolve_conflict_keep(&self, _: &str, _: &str, _: &str) -> Result<(), AppError> {
            Err(err("resolve_conflict_keep"))
        }
        async fn resolve_conflict_delete(&self, _: &str, _: &str) -> Result<(), AppError> {
            Err(err("resolve_conflict_delete"))
        }
        async fn operation_state(
            &self,
            _: &str,
        ) -> Result<Option<crate::core::engine::OperationState>, AppError> {
            Err(err("operation_state"))
        }
        async fn operation_abort(&self, _: &str) -> Result<(), AppError> {
            Err(err("operation_abort"))
        }
        async fn operation_continue(&self, _: &str) -> Result<(), AppError> {
            Err(err("operation_continue"))
        }
        async fn operation_skip(&self, _: &str) -> Result<(), AppError> {
            Err(err("operation_skip"))
        }
        async fn mergetool(
            &self,
            _: &str,
            _: &str,
            _: Option<&str>,
            _: Option<&str>,
        ) -> Result<(), AppError> {
            Err(err("mergetool"))
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

    #[tokio::test]
    async fn diff_cache_assigns_ids_and_expires_on_invalidate() {
        let engine = Arc::new(MockEngine {
            status_calls: AtomicUsize::new(0),
        });
        let mgr = RepoManager::new(engine as Arc<dyn GitEngine>);
        let dir = temp_repo();
        let id = mgr.open(dir.clone()).await.unwrap();

        let mut m1 = DiffModel {
            id: 0,
            source: DiffSource::Worktree,
            old_revision: None,
            new_revision: None,
            files: Vec::new(),
        };
        let mut m2 = m1.clone();
        mgr.cache_diff(id, &mut m1).await.unwrap();
        mgr.cache_diff(id, &mut m2).await.unwrap();
        assert_eq!(m1.id, 1);
        assert_eq!(m2.id, 2);
        assert!(mgr.get_diff(id, 1).await.is_ok());

        // Invalidation clears cached models (同源保证).
        mgr.invalidate(id, EventKinds::WORKTREE).await.unwrap();
        assert!(matches!(
            mgr.get_diff(id, 1).await,
            Err(AppError::DiffModelExpired)
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn diff_cache_evicts_oldest_beyond_cap() {
        let engine = Arc::new(MockEngine {
            status_calls: AtomicUsize::new(0),
        });
        let mgr = RepoManager::new(engine as Arc<dyn GitEngine>);
        let dir = temp_repo();
        let id = mgr.open(dir.clone()).await.unwrap();
        for _ in 0..10 {
            let mut m = DiffModel {
                id: 0,
                source: DiffSource::Worktree,
                old_revision: None,
                new_revision: None,
                files: Vec::new(),
            };
            mgr.cache_diff(id, &mut m).await.unwrap();
        }
        // Oldest ids evicted (cap 8), newest retained.
        assert!(mgr.get_diff(id, 1).await.is_err());
        assert!(mgr.get_diff(id, 3).await.is_ok());
        assert!(mgr.get_diff(id, 10).await.is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
