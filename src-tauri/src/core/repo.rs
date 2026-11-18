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
///
/// The gate also owns "写后失效" (PLAN §4.3, [`Self::mark_dirty`]): when the
/// last holder of a mutating operation drops its guard, the repository's
/// display cache is discarded. Without it, a caller that refreshes right after
/// its own write would read the *pre-write* snapshot — the cache is otherwise
/// only dropped when the watcher's debounced event finally lands
/// (300 ms–1 s later, plus a full re-read).
#[derive(Clone, Debug)]
pub struct WriteGate {
    repo_id: RepoId,
    inner: Arc<WriteGateInner>,
    sessions: Arc<Mutex<HashMap<RepoId, Session>>>,
}

#[derive(Debug)]
struct WriteGateInner {
    lock: Mutex<()>,
}

impl WriteGate {
    fn new(repo_id: RepoId, sessions: Arc<Mutex<HashMap<RepoId, Session>>>) -> Self {
        Self {
            repo_id,
            inner: Arc::new(WriteGateInner {
                lock: Mutex::new(()),
            }),
            sessions,
        }
    }

    /// Enter the gate. Dropping the returned guard makes the caches stale
    /// *before* the command's response reaches the caller.
    pub async fn lock(&self) -> WriteGuard<'_> {
        let guard = self.inner.lock.lock().await;
        WriteGuard {
            gate: self,
            _guard: guard,
        }
    }

    /// Wait until no mutating operation is in flight, **without** touching the
    /// display cache (unlike [`Self::lock`], whose guard marks it dirty).
    ///
    /// The watcher's host loop uses this so its invalidation never races our
    /// own write. An fs event caused by `git commit` lands while the commit is
    /// usually still running (debounce 300 ms ≈ commit duration); re-reading
    /// there competes with git for the index and its result is discarded
    /// moments later when the write guard bumps the generation (measured: a
    /// 276 ms commit grew to 637 ms, and the read itself was thrown away).
    pub async fn wait_idle(&self) {
        // FIFO-fair: this also queues behind writers that are already waiting,
        // so we observe the queue drained rather than a single gap.
        let _idle = self.inner.lock.lock().await;
    }

    /// Discard the display cache after a write.
    ///
    /// It only *drops* state — never fills it — so the direction of truth
    /// (`.git` → cache) is unchanged: the next read must go to git. That is
    /// why this is not the "内部写操作快速通道" the project forbids: nothing is
    /// written ahead of the watcher, the stale window is merely cut short.
    fn mark_dirty(&self) {
        let sessions = self.sessions.clone();
        let id = self.repo_id;
        // The write finished *now*: everything read before this instant may
        // predate it, so stamp it and let `status` discard superseded reads.
        let now = Instant::now();
        // Fast path: the lock is only ever held for a map lookup or a cache
        // store, so contention is rare and never worth blocking a command's
        // return on.
        let Ok(mut map) = sessions.try_lock() else {
            let Ok(handle) = tokio::runtime::Handle::try_current() else {
                return; // no runtime: the watcher's invalidation still covers us
            };
            handle.spawn(async move {
                let mut map = sessions.lock().await;
                if let Some(s) = map.get_mut(&id) {
                    s.last_change_at = Some(now);
                    bump_generation(s);
                }
            });
            return;
        };
        if let Some(s) = map.get_mut(&id) {
            s.last_change_at = Some(now);
            bump_generation(s);
        }
    }
}

/// Guard for a mutating operation; see [`WriteGate::mark_dirty`].
pub struct WriteGuard<'a> {
    gate: &'a WriteGate,
    _guard: tokio::sync::MutexGuard<'a, ()>,
}

impl Drop for WriteGuard<'_> {
    fn drop(&mut self) {
        self.gate.mark_dirty();
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

pub(crate) mod u64_as_string {
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
    /// Latest instant at which this repository's state is known to have
    /// changed: a watcher event's observation time, or the completion of one
    /// of our own writes (`WriteGate` guard drop). A status read that started
    /// before it may have raced the change; one that started after it did not.
    last_change_at: Option<Instant>,
    /// Start instant of the most recent `status` read (hit or miss). Lets an
    /// invalidation skip the speculative re-read when this read is already
    /// newer than the event that triggered it.
    last_read_started: Option<Instant>,
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

/// Bump the display-cache generation and drop every cached artifact.
/// Returns the new generation.
fn bump_generation(s: &mut Session) -> u64 {
    bump_generation_keeping(s, false)
}

/// [`bump_generation`] with the option to *keep* the status snapshot.
///
/// `keep_status` is passed when the snapshot provably already reflects the
/// change that triggered this invalidation (it was read after the newest fs
/// event). The generation still moves on: the graph cache is keyed by it and
/// a refs change must rebuild the graph even though the file list is current.
fn bump_generation_keeping(s: &mut Session, keep_status: bool) -> u64 {
    s.generation += 1;
    if !keep_status {
        s.status = None;
    }
    // DiffModels describe a point-in-time state; after any change they are
    // suspect and must not feed line-level patches.
    s.diffs.clear();
    // Graph cache key = refs generation → any event rebuilds (v1
    // 失效即全量重算).
    s.graph = None;
    s.generation
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
        let gate = self
            .gates
            .lock()
            .await
            .entry(id)
            .or_insert_with(|| WriteGate::new(id, self.sessions.clone()))
            .clone();
        Ok(gate)
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
    ///
    /// `observed_at` is the newest raw observation folded into this event
    /// batch. The 防抖重读 is *speculative* — it only exists to warm the cache
    /// before the frontend asks — so it is skipped when a status read that
    /// started after `observed_at` is already in flight or already cached:
    /// that read is at least as fresh as the re-read would be, and repeating
    /// it doubles the `git status` cost on exactly the path that matters
    /// (write → 前端刷新 → watcher 尾巴 event，大仓库下每次 ~300–1000 ms).
    pub async fn invalidate(
        &self,
        id: RepoId,
        kinds: EventKinds,
        observed_at: Instant,
    ) -> Option<u64> {
        let path = {
            let repos = self.repos.lock().await;
            repos.get(&id)?.clone()
        };
        let (generation, covered) = {
            let mut sessions = self.sessions.lock().await;
            let s = sessions.entry(id).or_default();
            // Never move the watermark backwards: a self-write completion can
            // be newer than the fs event that is only now reaching us.
            let prev = s.last_change_at;
            s.last_change_at = Some(match prev {
                Some(p) if p > observed_at => p,
                _ => observed_at,
            });
            let covered = s
                .last_read_started
                .is_some_and(|started| started > observed_at);
            let keep_status = covered && s.status.is_some();
            (bump_generation_keeping(s, keep_status), covered)
        };
        if covered {
            tracing::debug!(
                repo = id.0,
                ?kinds,
                generation,
                sla = "invalidate",
                "cache invalidated; re-read skipped (a read already covers this event)"
            );
            return Some(generation);
        }
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
                    // This read starts *after* the event, so it covers it —
                    // record it even if a newer invalidation supersedes the
                    // snapshot below.
                    s.last_read_started = Some(started);
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
        let started = Instant::now();
        let generation = {
            let mut sessions = self.sessions.lock().await;
            let s = sessions.entry(id).or_default();
            if let Some(snap) = s.status.as_ref() {
                return Ok(snap.status.clone());
            }
            s.last_read_started = Some(started);
            s.generation
        };
        let path = self
            .get_path(id)
            .await
            .ok_or_else(|| AppError::InvalidRepo {
                path: id.0.to_string(),
            })?;
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
        // A change newer than this read means the result may predate it (we
        // read while git was rewriting the index, or a write landed mid-read):
        // serve it to the caller but never cache it — the next read retries
        // instead of pinning a stale snapshot under the current generation.
        let superseded =
            s.generation != generation && s.last_change_at.is_some_and(|t| t > started);
        if !superseded {
            s.status = Some(StatusSnapshot {
                generation: s.generation,
                duration_ms,
                status: status.clone(),
            });
        }
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
        BackupRef, BlameResult, BranchInfo, CloneOptions, CommitInfo, CommitResult, CommitStatsDto,
        CommitTemplate, ConfigEntry, DiffModel, DiffSource, FileCommit, GitignoreFile, IndexEntry,
        NumstatCommit, PullResult, RebaseState, ReflogEntry, RemoteInfo, StashEntry, StashPushMode,
        TagInfo,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Minimal in-memory engine: counts status calls, fails everything else.
    struct MockEngine {
        status_calls: AtomicUsize,
        /// Artificial `git status` latency, for read/write race tests.
        status_delay: Option<std::time::Duration>,
    }

    impl MockEngine {
        fn new() -> Self {
            Self {
                status_calls: AtomicUsize::new(0),
                status_delay: None,
            }
        }

        fn slow(delay: std::time::Duration) -> Self {
            Self {
                status_calls: AtomicUsize::new(0),
                status_delay: Some(delay),
            }
        }
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
            if let Some(d) = self.status_delay {
                tokio::time::sleep(d).await;
            }
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
        async fn create_branch(
            &self,
            _: &str,
            _: &str,
            _: Option<&str>,
            _: bool,
        ) -> Result<(), AppError> {
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
        async fn list_remote_branches(&self, _: &str) -> Result<Vec<BranchInfo>, AppError> {
            Err(err("remote_branches"))
        }
        async fn checkout_remote_branch(&self, _: &str, _: &str, _: &str) -> Result<(), AppError> {
            Err(err("checkout_remote_branch"))
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
        async fn stash_push(
            &self,
            _: &str,
            _: Option<&str>,
            _: StashPushMode,
            _: bool,
        ) -> Result<usize, AppError> {
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

        async fn config_global(&self) -> Result<Vec<ConfigEntry>, AppError> {
            Err(err("config_global"))
        }
        async fn config_local(&self, _: &str) -> Result<Vec<ConfigEntry>, AppError> {
            Err(err("config_local"))
        }
        async fn config_merged(&self, _: &str) -> Result<Vec<ConfigEntry>, AppError> {
            Err(err("config_merged"))
        }
        async fn config_set_global(&self, _: &str, _: Option<&str>) -> Result<(), AppError> {
            Err(err("config_set_global"))
        }
        async fn config_set_local(
            &self,
            _: &str,
            _: &str,
            _: Option<&str>,
        ) -> Result<(), AppError> {
            Err(err("config_set_local"))
        }
        async fn global_gitignore(&self) -> Result<Option<GitignoreFile>, AppError> {
            Err(err("global_gitignore"))
        }
        async fn commit_template(&self, _: &str) -> Result<Option<CommitTemplate>, AppError> {
            Err(err("commit_template"))
        }
        async fn log_numstat(
            &self,
            _: &str,
            _: Option<&str>,
            _: Option<&str>,
            _: Option<&str>,
            _: u32,
        ) -> Result<Vec<NumstatCommit>, AppError> {
            Err(err("log_numstat"))
        }

        async fn commit_stats(&self, _: &str, _: &str) -> Result<CommitStatsDto, AppError> {
            Err(err("commit_stats"))
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
        let engine = Arc::new(MockEngine::new());
        let mgr = RepoManager::new(engine.clone() as Arc<dyn GitEngine>);
        let dir = temp_repo();
        let id = mgr.open(dir.clone()).await.unwrap();

        // Two reads → one engine call (cache hit on the second).
        let s1 = mgr.status(id).await.unwrap();
        let s2 = mgr.status(id).await.unwrap();
        assert_eq!(s1.len(), 1);
        assert_eq!(s1, s2);
        assert_eq!(engine.status_calls.load(Ordering::SeqCst), 1);

        // Invalidation re-reads immediately (防抖重读) → call 2. `observed_at`
        // is *now*, i.e. newer than the read above, so nothing covers it.
        let gen = mgr
            .invalidate(id, EventKinds::INDEX, Instant::now())
            .await
            .unwrap();
        assert_eq!(gen, 1);
        assert_eq!(engine.status_calls.load(Ordering::SeqCst), 2);

        // Subsequent read hits the fresh cache → still 2.
        let s3 = mgr.status(id).await.unwrap();
        assert_eq!(s3, s1);
        assert_eq!(engine.status_calls.load(Ordering::SeqCst), 2);

        // Generation monotonically increases per event.
        let gen2 = mgr
            .invalidate(id, EventKinds::WORKTREE, Instant::now())
            .await
            .unwrap();
        assert_eq!(gen2, 2);
        assert_eq!(engine.status_calls.load(Ordering::SeqCst), 3);

        mgr.close(id).await.unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 写后失效（PLAN §4.3）：写命令返回后缓存必然是脏的，下一次 status
    /// 必须穿透到 git —— 否则调用方紧随其后的刷新会读到写前的快照，直到
    /// watcher 防抖（300ms–1s）+ 全量重读才纠正（大仓库下肉眼可见）。
    #[tokio::test]
    async fn write_gate_drops_status_cache_on_guard_drop() {
        let engine = Arc::new(MockEngine::new());
        let mgr = RepoManager::new(engine.clone() as Arc<dyn GitEngine>);
        let dir = temp_repo();
        let id = mgr.open(dir.clone()).await.unwrap();

        let _ = mgr.status(id).await.unwrap(); // cold read → cached
        assert_eq!(engine.status_calls.load(Ordering::SeqCst), 1);

        let gate = mgr.write_gate(id).await.unwrap();
        {
            let _guard = gate.lock().await;
            // Inside the gate the pre-write cache is still served (a write
            // does not have to pay for a read it may not need).
            let _ = mgr.status(id).await.unwrap();
            assert_eq!(engine.status_calls.load(Ordering::SeqCst), 1);
        } // guard dropped → cache invalidated

        let _ = mgr.status(id).await.unwrap();
        assert_eq!(
            engine.status_calls.load(Ordering::SeqCst),
            2,
            "post-write read must penetrate to git, not serve the old snapshot"
        );

        mgr.close(id).await.unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 预读是投机性的（只为在 frontend 问之前把缓存焐热）：事件早于上一次
    /// 读取起点时，重读只会得到同一个答案。这正是"提交 → 前端刷新 → watcher
    /// 尾巴事件"的时序 —— 大仓库下重读是 300–1000 ms 的一次全量扫描。
    #[tokio::test]
    async fn invalidation_skips_reread_when_read_postdates_event() {
        let engine = Arc::new(MockEngine::new());
        let mgr = RepoManager::new(engine.clone() as Arc<dyn GitEngine>);
        let dir = temp_repo();
        let id = mgr.open(dir.clone()).await.unwrap();

        // Debounce 观测到的最后事件（早于下面的读取）.
        let event_at = Instant::now() - std::time::Duration::from_millis(5);
        let _ = mgr.status(id).await.unwrap();
        assert_eq!(engine.status_calls.load(Ordering::SeqCst), 1);

        let gen = mgr
            .invalidate(id, EventKinds::INDEX, event_at)
            .await
            .unwrap();
        assert_eq!(gen, 1, "generation still moves (graph cache keys on it)");
        assert_eq!(
            engine.status_calls.load(Ordering::SeqCst),
            1,
            "snapshot post-dates the event — the re-read would be redundant"
        );

        // 快照被保留 → 前端紧随其后的刷新命中缓存。
        let _ = mgr.status(id).await.unwrap();
        assert_eq!(engine.status_calls.load(Ordering::SeqCst), 1);

        mgr.close(id).await.unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 提交后的真实时序：前端刷新先发起读取，watcher 的尾巴事件晚到但
    /// *观测时刻早于*这次读取。此时既不该预读，也不能把在途结果丢掉 ——
    /// 否则前端会紧接着再扫一次全量 `git status`。
    #[tokio::test]
    async fn in_flight_read_covers_a_late_event() {
        let engine = Arc::new(MockEngine::slow(std::time::Duration::from_millis(120)));
        let mgr = RepoManager::new(engine.clone() as Arc<dyn GitEngine>);
        let dir = temp_repo();
        let id = mgr.open(dir.clone()).await.unwrap();

        let event_at = Instant::now() - std::time::Duration::from_millis(5);
        let reader = {
            let mgr = mgr.clone();
            tokio::spawn(async move { mgr.status(id).await })
        };
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;

        let gen = mgr
            .invalidate(id, EventKinds::INDEX, event_at)
            .await
            .unwrap();
        assert_eq!(gen, 1);
        assert_eq!(
            engine.status_calls.load(Ordering::SeqCst),
            1,
            "in-flight read started after the event → no speculative re-read"
        );

        reader.await.unwrap().unwrap();
        let _ = mgr.status(id).await.unwrap();
        assert_eq!(
            engine.status_calls.load(Ordering::SeqCst),
            1,
            "the in-flight read must be accepted, not discarded"
        );

        mgr.close(id).await.unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 读与写竞争：读取在写结束前开始，可能读到 index 的中间状态，结果
    /// 不得入缓存 —— 否则写后失效会被一次"看似成功"的写入抹掉。
    #[tokio::test]
    async fn read_racing_a_write_is_not_cached() {
        let engine = Arc::new(MockEngine::slow(std::time::Duration::from_millis(120)));
        let mgr = RepoManager::new(engine.clone() as Arc<dyn GitEngine>);
        let dir = temp_repo();
        let id = mgr.open(dir.clone()).await.unwrap();

        let reader = {
            let mgr = mgr.clone();
            tokio::spawn(async move { mgr.status(id).await })
        };
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        {
            let gate = mgr.write_gate(id).await.unwrap();
            let _guard = gate.lock().await;
        } // 写结束 → mark_dirty 把 last_change_at 推到"现在"

        reader.await.unwrap().unwrap();
        let _ = mgr.status(id).await.unwrap();
        assert_eq!(
            engine.status_calls.load(Ordering::SeqCst),
            2,
            "a read that started before the write must not seed the cache"
        );

        mgr.close(id).await.unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn invalidate_unknown_repo_returns_none() {
        let engine = Arc::new(MockEngine::new());
        let mgr = RepoManager::new(engine.clone() as Arc<dyn GitEngine>);
        assert!(mgr
            .invalidate(RepoId(999), EventKinds::INDEX, Instant::now())
            .await
            .is_none());
    }

    #[tokio::test]
    async fn write_gate_serializes_and_rejects_unknown_repo() {
        let engine = Arc::new(MockEngine::new());
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

    /// 回归（P10 提交体验）：提交后立刻读 status 必须是提交后的状态。
    ///
    /// 没有写后失效时，这里会命中提交前的快照（暂存条目还在），前端只能
    /// 等 watcher 防抖 + 全量重读才清空暂存区——大工作区下肉眼可见。
    #[tokio::test(flavor = "multi_thread")]
    async fn commit_then_status_is_immediately_post_commit() {
        use crate::core::engine::CliEngine;
        use crate::core::runner::GitProcessRunner;
        use std::process::Command;

        let dir = std::env::temp_dir().join(format!(
            "ibexgit-commit-status-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        for args in [
            vec!["init", "-q"],
            vec!["config", "user.email", "t@t"],
            vec!["config", "user.name", "t"],
        ] {
            assert!(Command::new("git")
                .args(&args)
                .current_dir(&dir)
                .status()
                .expect("git available")
                .success());
        }
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();

        let engine: Arc<dyn GitEngine> = Arc::new(CliEngine::new(GitProcessRunner::new(60), "git"));
        let mgr = RepoManager::new(engine);
        let id = mgr.open(dir.clone()).await.unwrap();
        let root = dir.display().to_string();

        // 暂存（写命令区间）→ 读一次 status（缓存里留下「1 个已暂存」）。
        {
            let gate = mgr.write_gate(id).await.unwrap();
            let _guard = gate.lock().await;
            mgr.engine()
                .stage(&root, &["a.txt".to_string()])
                .await
                .unwrap();
        }
        let staged = mgr.status(id).await.unwrap();
        assert_eq!(staged.len(), 1, "file should be staged before commit");

        // 提交（同一写闸门）→ 命令返回后立刻读。
        {
            let gate = mgr.write_gate(id).await.unwrap();
            let _guard = gate.lock().await;
            mgr.engine()
                .commit(&root, "first", false, false)
                .await
                .unwrap();
        }
        let after = mgr.status(id).await.unwrap();
        assert!(
            after.is_empty(),
            "status after commit must be empty (staged cleared), got {after:?}"
        );

        mgr.close(id).await.unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn read_permit_caps_parallelism() {
        let engine = Arc::new(MockEngine::new());
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
        let engine = Arc::new(MockEngine::new());
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
        let engine = Arc::new(MockEngine::new());
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
        mgr.invalidate(id, EventKinds::WORKTREE, Instant::now())
            .await
            .unwrap();
        assert!(matches!(
            mgr.get_diff(id, 1).await,
            Err(AppError::DiffModelExpired)
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn diff_cache_evicts_oldest_beyond_cap() {
        let engine = Arc::new(MockEngine::new());
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
