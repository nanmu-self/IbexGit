//! External-change detection: notify → classify → debounce → consolidated event.
//!
//! Flow (PLAN §4.3): fs event → [`classify_event`] → per-repo 300 ms trailing
//! debounce (1 s cap) → [`RepoEvent`] → host loop invalidates caches, re-reads
//! and emits `repo://changed` to the frontend.

use crate::core::error::AppError;
use crate::core::repo::RepoId;
use bitflags::bitflags;
use notify::{RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, RwLock};

/// Debounce window: events arriving within this period of the last one are
/// coalesced into a single consolidated event (PLAN §4.3: 300 ms).
pub const DEBOUNCE_WINDOW: Duration = Duration::from_millis(300);
/// Cap so continuous write bursts still produce events instead of starving.
pub const DEBOUNCE_MAX: Duration = Duration::from_millis(1000);

bitflags! {
    /// Which cache domains an fs event invalidates.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct EventKinds: u32 {
        const HEAD = 1;
        const INDEX = 1 << 1;
        const REFS = 1 << 2;
        const MERGE_STATE = 1 << 3;
        const WORKTREE = 1 << 4;
        const CONFIG = 1 << 5;
    }
}

impl EventKinds {
    /// Stable string names for the frontend payload.
    pub fn names(self) -> Vec<String> {
        let mut out = Vec::new();
        if self.contains(Self::HEAD) {
            out.push("head".into());
        }
        if self.contains(Self::INDEX) {
            out.push("index".into());
        }
        if self.contains(Self::REFS) {
            out.push("refs".into());
        }
        if self.contains(Self::MERGE_STATE) {
            out.push("merge_state".into());
        }
        if self.contains(Self::WORKTREE) {
            out.push("worktree".into());
        }
        if self.contains(Self::CONFIG) {
            out.push("config".into());
        }
        out
    }
}

/// Consolidated (post-debounce) change notification for one repository.
#[derive(Debug, Clone)]
pub struct RepoEvent {
    pub repo_id: RepoId,
    pub kinds: EventKinds,
}

/// Typed event delivered to the frontend after an invalidation cycle
/// completed and caches were re-read (tauri-specta generated bindings).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, specta::Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct RepoChanged {
    pub repo_id: RepoId,
    pub kinds: Vec<String>,
    pub generation: u32,
}

/// Locate the git dir for a worktree (handles the `.git` file of linked
/// worktrees: `gitdir: <path>`).
pub fn resolve_git_dir(worktree: &Path) -> Option<PathBuf> {
    let dot = worktree.join(".git");
    if dot.is_dir() {
        return Some(dot);
    }
    if dot.is_file() {
        let content = std::fs::read_to_string(&dot).ok()?;
        let target = content.lines().next()?.strip_prefix("gitdir:")?.trim();
        if target.is_empty() {
            return None;
        }
        let p = PathBuf::from(target);
        return Some(if p.is_absolute() { p } else { worktree.join(p) });
    }
    None
}

/// Classify one notify event into cache-invalidating kinds.
///
/// Pure function over absolute paths — unit-testable without spawning
/// watchers. Paths under `git_dir` map to git-internal domains, paths under
/// `worktree` to [`EventKinds::WORKTREE`]; chatty internals (objects, logs,
/// COMMIT_EDITMSG, …) are ignored.
pub fn classify_event(ev: &notify::Event, git_dir: &Path, worktree: &Path) -> EventKinds {
    if ev.need_rescan() {
        // Watcher lost track (buffer overflow / rescan request): invalidate all.
        return EventKinds::all();
    }
    match ev.kind {
        notify::EventKind::Access(_) | notify::EventKind::Other => return EventKinds::empty(),
        _ => {}
    }
    let mut kinds = EventKinds::empty();
    for p in &ev.paths {
        if let Ok(rel) = p.strip_prefix(git_dir) {
            kinds |= classify_git_rel(&rel.to_string_lossy().replace('\\', "/"));
        } else if p.starts_with(worktree) {
            // Any nested `.git` inside the worktree: ours is matched by the
            // git_dir branch above; other repos' internals are noise.
            let under_worktree = p.strip_prefix(worktree).unwrap_or(p.as_path());
            if under_worktree.components().any(|c| c.as_os_str() == ".git") {
                continue;
            }
            kinds |= EventKinds::WORKTREE;
        }
        // Paths outside both roots (temp files elsewhere) are ignored.
    }
    kinds
}

fn classify_git_rel(rel: &str) -> EventKinds {
    if rel == "HEAD" {
        return EventKinds::HEAD;
    }
    if rel.starts_with("refs/") || rel == "packed-refs" {
        return EventKinds::REFS;
    }
    if rel.starts_with("rebase-merge/")
        || rel.starts_with("rebase-apply/")
        || rel.starts_with("sequencer/")
    {
        return EventKinds::MERGE_STATE;
    }
    match rel {
        "index" => EventKinds::INDEX,
        "MERGE_HEAD" | "REBASE_HEAD" | "CHERRY_PICK_HEAD" | "REVERT_HEAD" | "BISECT_LOG" => {
            EventKinds::MERGE_STATE
        }
        "config" | "shallow" | "description" | "info/exclude" => EventKinds::CONFIG,
        // Noisy internals that don't change UI state.
        "COMMIT_EDITMSG" | "FETCH_HEAD" | "ORIG_HEAD" => EventKinds::empty(),
        _ if rel.starts_with("objects/")
            || rel.starts_with("logs/")
            || rel.starts_with("hooks/") =>
        {
            EventKinds::empty()
        }
        _ => EventKinds::empty(),
    }
}

/// Watches a single repository; raw (un-debounced) classifications go into
/// `tx`. Keep the watcher alive — dropping it stops watching.
struct RepoWatcher {
    _watcher: notify::RecommendedWatcher,
}

impl RepoWatcher {
    fn new(
        repo_id: RepoId,
        git_dir: PathBuf,
        worktree: PathBuf,
        tx: mpsc::UnboundedSender<EventKinds>,
    ) -> Result<Self, AppError> {
        let git_for_cb = git_dir.clone();
        let wt_for_cb = worktree.clone();
        let mut watcher =
            notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
                let started = Instant::now();
                let Ok(event) = res else { return };
                let kinds = classify_event(&event, &git_for_cb, &wt_for_cb);
                tracing::debug!(
                    repo = repo_id.0,
                    process_us = started.elapsed().as_micros() as u64,
                    ?kinds,
                    sla = "detect",
                    "fs event classified"
                );
                if !kinds.is_empty() {
                    let _ = tx.send(kinds);
                }
            })
            .map_err(|e| AppError::io_with_detail("watcher", e.to_string()))?;

        // git internals: top-level files (HEAD/index/MERGE_HEAD/…) plus refs/.
        let _ = watcher.watch(&git_dir, RecursiveMode::NonRecursive);
        let refs_dir = git_dir.join("refs");
        if refs_dir.is_dir() {
            let _ = watcher.watch(&refs_dir, RecursiveMode::Recursive);
        }
        let _ = watcher.watch(&worktree, RecursiveMode::Recursive);

        Ok(Self { _watcher: watcher })
    }
}

/// Central watcher hub: one watcher + one debounce task per open repository,
/// consolidating raw events into a single [`RepoEvent`] stream.
pub struct WatcherHub {
    tx: mpsc::UnboundedSender<RepoEvent>,
    rx: Mutex<Option<mpsc::UnboundedReceiver<RepoEvent>>>,
    watchers: RwLock<HashMap<RepoId, RepoWatcher>>,
}

impl WatcherHub {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            tx,
            rx: Mutex::new(Some(rx)),
            watchers: RwLock::new(HashMap::new()),
        }
    }

    /// Hand the consolidated event stream to the host (once).
    pub fn take_receiver(&self) -> Option<mpsc::UnboundedReceiver<RepoEvent>> {
        self.rx.lock().unwrap().take()
    }

    pub async fn add_repo(
        &self,
        repo_id: RepoId,
        git_dir: PathBuf,
        worktree: PathBuf,
    ) -> Result<(), AppError> {
        let (raw_tx, raw_rx) = mpsc::unbounded_channel();
        let watcher = RepoWatcher::new(repo_id, git_dir, worktree, raw_tx)?;
        let out_tx = self.tx.clone();
        tokio::spawn(debounce_loop(
            repo_id,
            raw_rx,
            out_tx,
            DEBOUNCE_WINDOW,
            DEBOUNCE_MAX,
        ));
        self.watchers.write().await.insert(repo_id, watcher);
        Ok(())
    }

    pub async fn remove_repo(&self, repo_id: RepoId) {
        // Dropping the watcher stops the notify side; the debounce task exits
        // when its raw channel closes.
        self.watchers.write().await.remove(&repo_id);
    }
}

impl Default for WatcherHub {
    fn default() -> Self {
        Self::new()
    }
}

/// Trailing debounce with a hard cap: each new event extends the deadline by
/// `window`, but never past `first_event + max`, so write bursts can't starve
/// the notification stream.
async fn debounce_loop(
    repo_id: RepoId,
    mut rx: mpsc::UnboundedReceiver<EventKinds>,
    tx: mpsc::UnboundedSender<RepoEvent>,
    window: Duration,
    max: Duration,
) {
    loop {
        let Some(mut kinds) = rx.recv().await else {
            return; // watcher dropped → raw channel closed
        };
        let started = Instant::now();
        let cap = started + max;
        let mut deadline = started + window;
        loop {
            if Instant::now() >= deadline {
                break;
            }
            let sleep = tokio::time::sleep_until(deadline.into());
            tokio::pin!(sleep);
            // Sleep expiry and channel closure would both surface as `None` on
            // their own — disambiguate so expiry breaks the window instead of
            // killing the debounce task.
            enum Step {
                Fire,
                More(EventKinds),
                Closed,
            }
            let step = tokio::select! {
                _ = &mut sleep => Step::Fire,
                ev = rx.recv() => match ev {
                    Some(k) => Step::More(k),
                    None => Step::Closed,
                },
            };
            match step {
                Step::Fire => break,
                Step::More(k) => {
                    kinds |= k;
                    let now = Instant::now();
                    deadline = deadline.max(now + window).min(cap);
                }
                Step::Closed => return,
            }
        }
        tracing::debug!(
            repo = repo_id.0,
            window_ms = started.elapsed().as_millis() as u64,
            ?kinds,
            sla = "debounce",
            "consolidated change event"
        );
        let _ = tx.send(RepoEvent { repo_id, kinds });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{DataChange, ModifyKind};

    fn root() -> PathBuf {
        PathBuf::from("/repo")
    }

    fn modify_event(paths: Vec<PathBuf>) -> notify::Event {
        let mut ev =
            notify::Event::new(notify::EventKind::Modify(ModifyKind::Data(DataChange::Any)));
        ev.paths = paths;
        ev
    }

    #[test]
    fn classify_head_index_refs() {
        let r = root();
        let git = r.join(".git");
        let wt = r;

        let ev = modify_event(vec![git.join("HEAD")]);
        assert_eq!(classify_event(&ev, &git, &wt), EventKinds::HEAD);

        let ev = modify_event(vec![git.join("index")]);
        assert_eq!(classify_event(&ev, &git, &wt), EventKinds::INDEX);

        let ev = modify_event(vec![git.join("refs/heads/main")]);
        assert_eq!(classify_event(&ev, &git, &wt), EventKinds::REFS);

        let ev = modify_event(vec![git.join("packed-refs")]);
        assert_eq!(classify_event(&ev, &git, &wt), EventKinds::REFS);
    }

    #[test]
    fn classify_merge_state() {
        let r = root();
        let git = r.join(".git");
        let wt = r;
        for f in [
            "MERGE_HEAD",
            "REBASE_HEAD",
            "CHERRY_PICK_HEAD",
            "rebase-merge/msgnum",
        ] {
            let ev = modify_event(vec![git.join(f)]);
            assert_eq!(
                classify_event(&ev, &git, &wt),
                EventKinds::MERGE_STATE,
                "file: {f}"
            );
        }
    }

    #[test]
    fn classify_worktree_and_noise() {
        let r = root();
        let git = r.join(".git");
        let wt = r;

        let ev = modify_event(vec![wt.join("src/main.rs")]);
        assert_eq!(classify_event(&ev, &git, &wt), EventKinds::WORKTREE);

        // Nested repo's .git inside worktree → not a worktree change.
        let ev = modify_event(vec![wt.join("sub/.git/index")]);
        assert_eq!(classify_event(&ev, &git, &wt), EventKinds::empty());

        // Chatty internals ignored.
        for f in [
            "objects/ab/cdef",
            "logs/HEAD",
            "COMMIT_EDITMSG",
            "FETCH_HEAD",
        ] {
            let ev = modify_event(vec![git.join(f)]);
            assert_eq!(
                classify_event(&ev, &git, &wt),
                EventKinds::empty(),
                "file: {f}"
            );
        }

        // Outside both roots → ignored.
        let ev = modify_event(vec![PathBuf::from("/elsewhere/x")]);
        assert_eq!(classify_event(&ev, &git, &wt), EventKinds::empty());
    }

    #[test]
    fn classify_config_and_multi_path_merge() {
        let r = root();
        let git = r.join(".git");
        let wt = r;

        let ev = modify_event(vec![git.join("config")]);
        assert_eq!(classify_event(&ev, &git, &wt), EventKinds::CONFIG);

        // One event touching several domains merges kinds.
        let ev = modify_event(vec![git.join("HEAD"), git.join("index")]);
        assert_eq!(
            classify_event(&ev, &git, &wt),
            EventKinds::HEAD | EventKinds::INDEX
        );
    }

    #[test]
    fn classify_access_events_ignored() {
        let r = root();
        let git = r.join(".git");
        let wt = r;
        let ev = notify::Event::new(notify::EventKind::Access(notify::event::AccessKind::Close(
            notify::event::AccessMode::Write,
        )));
        let mut ev = ev;
        ev.paths = vec![git.join("index")];
        assert_eq!(classify_event(&ev, &git, &wt), EventKinds::empty());
    }

    #[test]
    fn kinds_names_roundtrip() {
        let k = EventKinds::HEAD | EventKinds::WORKTREE;
        assert_eq!(k.names(), vec!["head".to_string(), "worktree".to_string()]);
        assert_eq!(EventKinds::empty().names(), Vec::<&str>::new());
    }

    #[tokio::test(start_paused = true)]
    async fn debounce_consolidates_burst() {
        let (tx, rx) = mpsc::unbounded_channel();
        let (out_tx, mut out_rx) = mpsc::unbounded_channel();
        tokio::spawn(debounce_loop(
            RepoId(1),
            rx,
            out_tx,
            Duration::from_millis(300),
            Duration::from_millis(1000),
        ));

        tx.send(EventKinds::INDEX).unwrap();
        tokio::time::advance(Duration::from_millis(100)).await;
        tx.send(EventKinds::WORKTREE).unwrap();
        tokio::time::advance(Duration::from_millis(100)).await;
        tx.send(EventKinds::HEAD).unwrap();

        // 300 ms of quiet after the last event → consolidated fire.
        tokio::time::advance(Duration::from_millis(350)).await;
        let ev = tokio::time::timeout(Duration::from_millis(50), out_rx.recv())
            .await
            .expect("event should fire after quiet window")
            .expect("channel open");
        assert_eq!(ev.repo_id, RepoId(1));
        assert_eq!(
            ev.kinds,
            EventKinds::INDEX | EventKinds::WORKTREE | EventKinds::HEAD
        );
    }

    #[tokio::test(start_paused = true)]
    async fn debounce_caps_max_window() {
        let (tx, rx) = mpsc::unbounded_channel();
        let (out_tx, mut out_rx) = mpsc::unbounded_channel();
        tokio::spawn(debounce_loop(
            RepoId(2),
            rx,
            out_tx,
            Duration::from_millis(300),
            Duration::from_millis(1000),
        ));

        // Continuous writes every 200 ms — deadline keeps extending, but the
        // 1 s cap must fire the event by ~1000 ms instead of starving.
        for i in 0..5 {
            tx.send(EventKinds::WORKTREE).unwrap();
            tokio::time::advance(Duration::from_millis(200)).await;
            let _ = i;
        }
        tokio::time::advance(Duration::from_millis(250)).await; // t ≈ 1250ms > cap
        let ev = tokio::time::timeout(Duration::from_millis(50), out_rx.recv())
            .await
            .expect("capped event must fire despite continuous writes")
            .expect("channel open");
        assert_eq!(ev.kinds, EventKinds::WORKTREE);
    }

    #[tokio::test(start_paused = true)]
    async fn debounce_separate_windows_fire_separately() {
        let (tx, rx) = mpsc::unbounded_channel();
        let (out_tx, mut out_rx) = mpsc::unbounded_channel();
        tokio::spawn(debounce_loop(
            RepoId(3),
            rx,
            out_tx,
            Duration::from_millis(300),
            Duration::from_millis(1000),
        ));

        tx.send(EventKinds::INDEX).unwrap();
        tokio::time::advance(Duration::from_millis(400)).await;
        let first = out_rx.recv().await.expect("first window fires");
        assert_eq!(first.kinds, EventKinds::INDEX);

        // A later, unrelated event starts a new window.
        tx.send(EventKinds::REFS).unwrap();
        tokio::time::advance(Duration::from_millis(400)).await;
        let second = out_rx.recv().await.expect("second window fires");
        assert_eq!(second.kinds, EventKinds::REFS);
    }

    #[test]
    fn resolve_git_dir_from_file() {
        let tmp = std::env::temp_dir().join(format!("ibexgit-wt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        // Linked-worktree style: .git is a file pointing elsewhere.
        let real = tmp.join("real-git-dir");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(tmp.join(".git"), format!("gitdir: {}\n", real.display())).unwrap();
        let resolved = resolve_git_dir(&tmp).expect("should resolve");
        assert_eq!(resolved, real);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}

#[cfg(test)]
mod integration {
    use super::*;
    use std::process::Command;
    use std::time::Duration;

    /// End-to-end smoke: real git repo + real fs events through the full
    /// chain (notify → classify → debounce → consolidated event).
    #[tokio::test]
    async fn watcher_detects_external_worktree_change() {
        let dir = std::env::temp_dir().join(format!(
            "ibexgit-watcher-e2e-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let init = Command::new("git")
            .args(["init", "-q"])
            .current_dir(&dir)
            .status()
            .expect("git available");
        assert!(init.success(), "git init failed");

        let repo_id = RepoId(7);
        let hub = WatcherHub::new();
        let git_dir = resolve_git_dir(&dir).expect("git dir");
        hub.add_repo(repo_id, git_dir, dir.clone()).await.unwrap();
        let mut rx = hub.take_receiver().expect("receiver");

        // External tool writes into the worktree.
        std::fs::write(dir.join("external-change.txt"), "hello").unwrap();

        let ev = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("event within 10s")
            .expect("channel open");
        assert_eq!(ev.repo_id, repo_id);
        assert!(
            ev.kinds.contains(EventKinds::WORKTREE),
            "expected WORKTREE kind, got {:?}",
            ev.kinds
        );

        hub.remove_repo(repo_id).await;
        let _ = std::fs::remove_dir_all(&dir);
    }
}
