//! DiscardRecovery — Recovery System 轨道 A（PLAN §4.7，P3）。
//!
//! 丢弃工作区改动前的完整快照，使"丢弃"可撤销。快照记录三类信息：
//!
//! 1. **index 态**：受影响路径的 `git ls-files -s` 记录（mode/sha/stage，
//!    含未合并 stage 1..3）。blob 一旦进过 index 就必然存在于对象库，
//!    因此恢复是确定性的——重建 index 后 `git status` 立即回到快照时的
//!    staged 状态（rename、binary、delete 全部覆盖）。
//! 2. **worktree 内容**：每个受影响路径的物理副本（带单路径大小上限，
//!    超限仅警告不阻塞）；快照时已删除的文件记录"不存在"，恢复时重新删除。
//! 3. **untracked 文件/目录**：与 worktree 副本同机制（git 无法用 patch
//!    表达，也不会被 `git restore` 触及）。
//!
//! 存储布局（PLAN §4.4）：
//! `{appData}/recovery/{repoHash}/{ts_ms}-{nonce}/`
//!   ├── meta.json     — `DiscardMeta`（serde JSON）
//!   └── files/{n}     — worktree/untracked 物理副本（文件或目录）
//!
//! 保留策略：默认 7 天，列表/快照时惰性清理过期快照。
//!
//! 设计取舍（ADR-012）：PLAN 原案为"双 patch（staged/unstaged）+ untracked
//! 副本"，但 patch 反向 apply 存在 worktree 上下文漂移失败模式（scope=all
//! 丢弃后 worktree=HEAD，反向应用 unstaged patch 的 preimage 是当时的
//! index 态，两边对不上），且文本 patch 天然不含二进制内容。改用
//! "index blob + 物理副本"：字节级精确、无上下文匹配、实现更简单。

use crate::core::engine::{GitEngine, IndexEntry};
use crate::core::error::AppError;
use crate::core::repo::RepoId;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Per-path copy size cap: larger paths are not copied; the snapshot keeps
/// a warning and the UI surfaces it before/after restore.
pub const MAX_COPY_BYTES: u64 = 10 * 1024 * 1024;

/// Snapshot retention window (PLAN §4.7: 如 7 天).
pub const RETENTION: Duration = Duration::from_secs(7 * 24 * 3600);

/// Backup ref namespace (PLAN §4.7 轨道 B): Recovery Points live under
/// `refs/ibexgit/backups/<op>-<ts>` — while the ref exists, the referenced
/// commits stay reachable and safe from GC; cleanup deletes the ref and
/// leaves the rest to normal Git garbage collection.
pub const BACKUP_REF_PREFIX: &str = "refs/ibexgit/backups/";

/// The discard scope a snapshot was taken for (mirrors the command param).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscardScope {
    /// Drop worktree changes only (staged state is kept).
    Worktree,
    /// Drop staged + worktree changes (back to HEAD).
    All,
}

impl DiscardScope {
    pub fn as_str(self) -> &'static str {
        match self {
            DiscardScope::Worktree => "worktree",
            DiscardScope::All => "all",
        }
    }
}

/// One snapshotted path.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct MetaEntry {
    path: String,
    untracked: bool,
    /// `ls-files -s` records at snapshot time (all stages; empty for
    /// untracked paths).
    index: Vec<IndexEntry>,
    /// Physical worktree copy, if the path existed and fit the size cap.
    /// `None` means the path did not exist at snapshot time (restore
    /// deletes it) — unless [`MetaEntry::over_limit`] is set (restore
    /// leaves the path untouched).
    worktree: Option<WorktreeCopy>,
    /// The path existed but exceeded [`MAX_COPY_BYTES`]: no copy kept.
    over_limit: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorktreeCopy {
    /// Name inside the snapshot dir (`files/{n}`); may be a directory.
    file: String,
    size: u64,
    is_dir: bool,
}

/// Internal metadata persisted as `meta.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct DiscardMeta {
    version: u32,
    kind: String, // "discard"
    scope: DiscardScope,
    repo_path: String,
    created_at_ms: u64,
    entries: Vec<MetaEntry>,
    warnings: Vec<String>,
}

/// Frontend-facing summary of one snapshot (specta type).
/// Timestamps/counts use f64 on the wire: u64 is JS-precision-unsafe and
/// specta forbids it; all values stay below 2^53.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, specta::Type)]
pub struct RecoveryEntry {
    pub id: String,
    pub created_at_ms: f64,
    pub kind: String,
    pub scope: String,
    pub file_count: u32,
    pub size_bytes: f64,
    pub warnings: Vec<String>,
}

/// Result of a successful snapshot.
pub struct Snapshot {
    pub id: String,
}

/// A path selected for discard, pre-classified by the caller (the command
/// holds a fresh status classification under the write gate).
#[derive(Debug, Clone)]
pub struct DiscardTarget {
    pub path: String,
    pub untracked: bool,
    /// Unmerged in the index (stage > 0): discard means full reset to HEAD.
    pub conflict: bool,
}

/// RecoveryManager: snapshots for track-A recovery under `{appData}/recovery`.
#[derive(Clone)]
pub struct RecoveryManager {
    root: PathBuf,
}

impl RecoveryManager {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Per-repo directory, hashed like [`RepoId`] (no path leakage into
    /// user-visible directory names).
    fn repo_dir(&self, repo_path: &Path) -> PathBuf {
        self.root.join(format!("{:016x}", RepoId::new(repo_path).0))
    }

    fn snapshot_dir(&self, repo_path: &Path, id: &str) -> PathBuf {
        self.repo_dir(repo_path).join(id)
    }

    /// Snapshot everything the upcoming discard will destroy. Must be called
    /// while holding the repo write gate (git reads + fs copies are
    /// serialized against concurrent mutations).
    pub async fn snapshot_discard(
        &self,
        engine: &dyn GitEngine,
        repo_path: &Path,
        targets: &[DiscardTarget],
        scope: DiscardScope,
    ) -> Result<Snapshot, AppError> {
        if targets.is_empty() {
            return Err(AppError::internal("no discard targets to snapshot"));
        }
        let tracked: Vec<String> = targets
            .iter()
            .filter(|t| !t.untracked)
            .map(|t| t.path.clone())
            .collect();
        let index = engine
            .ls_index(&repo_path.display().to_string(), &tracked)
            .await?;

        let dir = self.create_snapshot_dir(repo_path)?;
        let result = Self::write_snapshot(&dir, repo_path, targets, scope, &index);
        match result {
            Ok(id) => Ok(Snapshot { id }),
            Err(e) => {
                let _ = std::fs::remove_dir_all(&dir);
                Err(e)
            }
        }
    }

    fn write_snapshot(
        dir: &Path,
        repo_path: &Path,
        targets: &[DiscardTarget],
        scope: DiscardScope,
        index: &[IndexEntry],
    ) -> Result<String, AppError> {
        let files_dir = dir.join("files");
        std::fs::create_dir_all(&files_dir)?;
        let mut entries = Vec::new();
        let mut warnings = Vec::new();
        let mut copy_idx = 0usize;
        for target in targets {
            let rel = target.path.trim_end_matches('/');
            let abs = repo_path.join(rel);
            let mut entry = MetaEntry {
                path: target.path.clone(),
                untracked: target.untracked,
                index: index
                    .iter()
                    .filter(|e| e.path == target.path)
                    .cloned()
                    .collect(),
                worktree: None,
                over_limit: false,
            };
            match std::fs::symlink_metadata(&abs) {
                Ok(meta) => {
                    let is_dir = meta.is_dir();
                    let name = format!("{copy_idx}");
                    let dest = files_dir.join(&name);
                    let size = if is_dir {
                        copy_dir(&abs, &dest)?
                    } else {
                        std::fs::copy(&abs, &dest)?
                    };
                    if size > MAX_COPY_BYTES {
                        let _ = if dest.is_dir() {
                            std::fs::remove_dir_all(&dest)
                        } else {
                            std::fs::remove_file(&dest)
                        };
                        entry.over_limit = true;
                        warnings.push(format!(
                            "{}: {:.1} MB 超过快照大小上限（{} MB），恢复时无法还原该路径内容",
                            target.path,
                            size as f64 / 1024.0 / 1024.0,
                            MAX_COPY_BYTES / 1024 / 1024
                        ));
                    } else {
                        entry.worktree = Some(WorktreeCopy {
                            file: format!("files/{name}"),
                            size,
                            is_dir,
                        });
                        copy_idx += 1;
                    }
                }
                // Path absent at snapshot time: restore re-deletes it.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
            entries.push(entry);
        }

        let meta = DiscardMeta {
            version: 1,
            kind: "discard".into(),
            scope,
            repo_path: repo_path.display().to_string(),
            created_at_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            entries,
            warnings,
        };
        let json = serde_json::to_vec_pretty(&meta)?;
        std::fs::write(dir.join("meta.json"), json)?;
        Ok(dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string())
    }

    fn read_meta(&self, repo_path: &Path, id: &str) -> Result<DiscardMeta, AppError> {
        validate_id(id)?;
        let bytes = std::fs::read(self.snapshot_dir(repo_path, id).join("meta.json"))?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    /// Create a track-B recovery point (backup ref) at `target` (usually
    /// `HEAD`) before a dangerous operation. Must be called while holding
    /// the repo write gate. Returns the full refname.
    pub async fn create_backup(
        &self,
        engine: &dyn GitEngine,
        repo_path: &Path,
        op: &str,
        target: &str,
    ) -> Result<String, AppError> {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let name = format!("{}{}-{}", BACKUP_REF_PREFIX, op, ts);
        engine
            .update_ref(&repo_path.display().to_string(), &name, target)
            .await?;
        Ok(name)
    }

    /// All track-B recovery points (oldest first).
    pub async fn list_backups(
        &self,
        engine: &dyn GitEngine,
        repo_path: &Path,
    ) -> Result<Vec<crate::core::engine::BackupRef>, AppError> {
        engine
            .list_backup_refs(&repo_path.display().to_string())
            .await
    }

    /// Delete the given backup refs (孤儿备份清理入口). The referenced
    /// commits become garbage-collectable again.
    pub async fn delete_backups(
        &self,
        engine: &dyn GitEngine,
        repo_path: &Path,
        names: &[String],
    ) -> Result<(), AppError> {
        engine
            .delete_backup_refs(&repo_path.display().to_string(), names)
            .await
    }

    /// List snapshots for a repo (newest first), pruning expired ones first.
    pub fn list(&self, repo_path: &Path) -> Vec<RecoveryEntry> {
        self.prune_expired(repo_path);
        let dir = self.repo_dir(repo_path);
        let Ok(read) = std::fs::read_dir(&dir) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for entry in read.flatten() {
            let Some(id) = entry.file_name().to_str().map(|s| s.to_string()) else {
                continue;
            };
            let Ok(meta) = self.read_meta(repo_path, &id) else {
                continue;
            };
            out.push(RecoveryEntry {
                id,
                created_at_ms: meta.created_at_ms as f64,
                kind: meta.kind,
                scope: meta.scope.as_str().to_string(),
                file_count: meta.entries.len() as u32,
                size_bytes: dir_size(&entry.path()) as f64,
                warnings: meta.warnings,
            });
        }
        out.sort_by(|a, b| b.created_at_ms.total_cmp(&a.created_at_ms));
        out
    }

    /// Restore a discard snapshot: rebuild the exact index state, then put
    /// worktree copies back (and re-delete files that were deleted). Must be
    /// called under the repo write gate.
    pub async fn restore_discard(
        &self,
        engine: &dyn GitEngine,
        repo_path: &Path,
        id: &str,
    ) -> Result<(), AppError> {
        let meta = self.read_meta(repo_path, id)?;
        if meta.kind != "discard" {
            return Err(AppError::internal(format!(
                "unsupported recovery kind: {}",
                meta.kind
            )));
        }
        let repo_str = repo_path.display().to_string();

        // 0) Clear the CURRENT index state of every target path first —
        //    `update-index --index-info` keeps an existing stage-0 entry
        //    when unmerged stages arrive, which would corrupt the state.
        //    The snapshot defines the truth: whatever it recorded (possibly
        //    nothing, e.g. rename sources) gets written back verbatim.
        let clear: Vec<String> = meta.entries.iter().map(|e| e.path.clone()).collect();
        engine.remove_index_entries(&repo_str, &clear).await?;

        // 1) Index: tracked paths — write back all recorded stages.
        let mut info = String::new();
        for entry in &meta.entries {
            for e in &entry.index {
                // `update-index --index-info -z` record format.
                info.push_str(&format!("{} {} {}\t{}\0", e.mode, e.sha, e.stage, e.path));
            }
        }
        engine.update_index_info(&repo_str, &info).await?;

        // 2) Worktree: copies back; paths absent at snapshot time get
        //    deleted again; over-cap paths are left untouched (warning was
        //    surfaced at snapshot time).
        let snapshot_dir = self.snapshot_dir(repo_path, id);
        for entry in &meta.entries {
            let rel = entry.path.trim_end_matches('/');
            let abs = repo_path.join(rel);
            match &entry.worktree {
                Some(copy) => {
                    if let Some(parent) = abs.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    if copy.is_dir {
                        if abs.symlink_metadata().is_ok() {
                            remove_path(&abs)?;
                        }
                        std::fs::create_dir_all(&abs)?;
                        copy_dir(&snapshot_dir.join(&copy.file), &abs)?;
                    } else {
                        std::fs::copy(snapshot_dir.join(&copy.file), &abs)?;
                    }
                }
                None => {
                    if entry.over_limit {
                        continue;
                    }
                    if abs.symlink_metadata().is_ok() {
                        remove_path(&abs)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Delete one snapshot (after a successful restore, or manual cleanup).
    pub fn delete(&self, repo_path: &Path, id: &str) -> Result<(), AppError> {
        validate_id(id)?;
        let dir = self.snapshot_dir(repo_path, id);
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
        Ok(())
    }

    /// Remove snapshots older than [`RETENTION`].
    pub fn prune_expired(&self, repo_path: &Path) {
        let dir = self.repo_dir(repo_path);
        let Ok(read) = std::fs::read_dir(&dir) else {
            return;
        };
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let cutoff = now_ms.saturating_sub(RETENTION.as_millis() as u64);
        for entry in read.flatten() {
            let file_name = entry.file_name();
            let Some(id) = file_name.to_str() else {
                continue;
            };
            if validate_id(id).is_err() {
                continue;
            }
            // Timestamp prefix of the dir name decides age (no meta parse
            // needed; malformed dirs are left alone).
            let Some(ts) = id.split('-').next().and_then(|s| s.parse::<u64>().ok()) else {
                continue;
            };
            if ts < cutoff {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }

    fn create_snapshot_dir(&self, repo_path: &Path) -> Result<PathBuf, AppError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let base = self.repo_dir(repo_path);
        std::fs::create_dir_all(&base)?;
        // `{ts_ms}-{sub-ms nonce}`; create_dir fails on collision → retry.
        for attempt in 0..10u32 {
            let id = format!(
                "{}-{}",
                now.as_millis(),
                now.subsec_nanos() as u64 + attempt as u64
            );
            let dir = base.join(&id);
            match std::fs::create_dir(&dir) {
                Ok(()) => return Ok(dir),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            }
        }
        Err(AppError::internal("failed to allocate snapshot id"))
    }
}

/// Recursive copy for untracked directories; returns the total copied size.
fn copy_dir(src: &Path, dest: &Path) -> Result<u64, AppError> {
    std::fs::create_dir_all(dest)?;
    let mut total = 0u64;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ft = entry.file_type()?;
        let to = dest.join(entry.file_name());
        if ft.is_dir() {
            total += copy_dir(&entry.path(), &to)?;
        } else {
            // Symlinks are followed: recovery favors data over link fidelity.
            total += std::fs::copy(entry.path(), &to)?;
        }
    }
    Ok(total)
}

fn remove_path(path: &Path) -> Result<(), AppError> {
    if path.is_dir() && !path.is_symlink() {
        std::fs::remove_dir_all(path)?;
    } else {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

fn validate_id(id: &str) -> Result<(), AppError> {
    let ok = !id.is_empty()
        && id.len() <= 64
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
    if ok {
        Ok(())
    } else {
        Err(AppError::internal(format!("invalid snapshot id: {id}")))
    }
}

fn dir_size(path: &Path) -> u64 {
    let mut total = 0;
    if let Ok(read) = std::fs::read_dir(path) {
        for entry in read.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_dir() {
                total += dir_size(&entry.path());
            } else if let Ok(meta) = entry.metadata() {
                total += meta.len();
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::engine::CliEngine;
    use crate::core::runner::GitProcessRunner;
    use std::process::Command;

    struct TempRepo {
        dir: PathBuf,
    }

    impl TempRepo {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "ibexgit-recovery-{}-{}",
                tag,
                std::time::SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            git(&dir, &["init", "-q", "-b", "main"]);
            git(&dir, &["config", "user.email", "t@t"]);
            git(&dir, &["config", "user.name", "t"]);
            // Deterministic byte-level content: no smudge/clean filters.
            git(&dir, &["config", "core.autocrlf", "false"]);
            TempRepo { dir }
        }

        fn write(&self, rel: &str, content: &[u8]) {
            let path = self.dir.join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(path, content).unwrap();
        }

        fn read(&self, rel: &str) -> Option<Vec<u8>> {
            std::fs::read(self.dir.join(rel)).ok()
        }

        fn engine(&self) -> CliEngine {
            CliEngine::new(GitProcessRunner::new(60), "git")
        }

        fn path_str(&self) -> String {
            self.dir.display().to_string()
        }

        /// Classify requested paths against fresh status output, expanding
        /// staged-rename original paths exactly like the git_discard command.
        fn classify(
            &self,
            status: &[crate::core::engine::FileStatus],
            paths: &[&str],
        ) -> Vec<DiscardTarget> {
            let mut out: Vec<DiscardTarget> = Vec::new();
            for p in paths {
                let Some(f) = status.iter().find(|f| &f.path == p) else {
                    continue;
                };
                out.push(DiscardTarget {
                    path: p.to_string(),
                    untracked: f.untracked,
                    conflict: f.conflict,
                });
                if f.staged && !f.untracked {
                    if let Some(orig) = &f.orig_path {
                        if !out.iter().any(|t| t.path == *orig) {
                            out.push(DiscardTarget {
                                path: orig.clone(),
                                untracked: false,
                                conflict: false,
                            });
                        }
                    }
                }
            }
            out
        }
    }

    impl Drop for TempRepo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args(["-C", dir.to_str().unwrap()])
            .args(args)
            .output()
            .expect("git available");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn mgr(tag: &str) -> RecoveryManager {
        let root = std::env::temp_dir().join(format!(
            "ibexgit-recovery-root-{}-{}",
            tag,
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        RecoveryManager::new(root)
    }

    fn head_hash(repo: &TempRepo) -> String {
        let out = Command::new("git")
            .args(["-C", repo.dir.to_str().unwrap(), "rev-parse", "HEAD"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// The P3 acceptance case: staged + unstaged + untracked + deletion +
    /// rename + binary, all discarded in one go, then fully restored.
    #[tokio::test]
    async fn discard_and_restore_combined_state() {
        let repo = TempRepo::new("combined");
        let engine = repo.engine();
        let recovery = mgr("combined");

        repo.write("base.txt", b"base line\n");
        repo.write("del.txt", b"to be deleted\n");
        repo.write("renamed_old.txt", b"rename me\n");
        git(&repo.dir, &["add", "-A"]);
        git(&repo.dir, &["commit", "-q", "-m", "init"]);
        let head = head_hash(&repo);

        // Build the combo state:
        //  - staged.txt: staged add
        repo.write("staged.txt", b"staged content\n");
        git(&repo.dir, &["add", "staged.txt"]);
        //  - base.txt: staged edit + further unstaged edit on top
        repo.write("base.txt", b"base line\nstaged edit\n");
        git(&repo.dir, &["add", "base.txt"]);
        repo.write("base.txt", b"base line\nstaged edit\nworktree edit\n");
        //  - del.txt: worktree deletion (tracked)
        std::fs::remove_file(repo.dir.join("del.txt")).unwrap();
        //  - rename: staged rename with additional unstaged content change
        repo.write("renamed_old.txt", b"rename me\nmore\n");
        git(&repo.dir, &["add", "renamed_old.txt"]);
        git(&repo.dir, &["mv", "renamed_old.txt", "renamed_new.txt"]);
        repo.write("renamed_new.txt", b"rename me\nmore\nunstaged tail\n");
        //  - untracked.txt + untracked dir
        repo.write("untracked.txt", b"untracked content\n");
        repo.write("untracked_dir/nested.txt", b"nested\n");
        //  - binary.bin: staged binary change
        repo.write("binary.bin", &[0u8, 159, 146, 150, 0, 1, 2, 255]);
        git(&repo.dir, &["add", "binary.bin"]);

        let status = engine.status(&repo.path_str()).await.unwrap();
        let targets = repo.classify(
            &status,
            &[
                "staged.txt",
                "base.txt",
                "del.txt",
                "renamed_new.txt",
                "renamed_old.txt",
                "untracked.txt",
                // `-uall` expands the untracked directory into per-file entries.
                "untracked_dir/nested.txt",
                "binary.bin",
            ],
        );
        assert_eq!(targets.len(), 8);

        // Snapshot, then discard everything (scope=all).
        let snap = recovery
            .snapshot_discard(&engine, &repo.dir, &targets, DiscardScope::All)
            .await
            .unwrap();

        engine
            .restore_to_head(
                &repo.path_str(),
                &[
                    "staged.txt".into(),
                    "base.txt".into(),
                    "del.txt".into(),
                    "renamed_new.txt".into(),
                    "renamed_old.txt".into(),
                    "binary.bin".into(),
                ],
            )
            .await
            .unwrap();
        engine
            .delete_untracked(
                &repo.path_str(),
                &["untracked.txt".into(), "untracked_dir/".into()],
            )
            .await
            .unwrap();

        let status = engine.status(&repo.path_str()).await.unwrap();
        assert!(status.is_empty(), "expected clean, got {status:?}");
        assert_eq!(head_hash(&repo), head);

        // Restore the snapshot → the exact combo state returns.
        recovery
            .restore_discard(&engine, &repo.dir, &snap.id)
            .await
            .unwrap();

        assert_eq!(
            repo.read("staged.txt").unwrap(),
            b"staged content\n",
            "staged-new file restored"
        );
        assert_eq!(
            repo.read("base.txt").unwrap(),
            b"base line\nstaged edit\nworktree edit\n",
            "staged+unstaged combo restored"
        );
        assert!(
            !repo.dir.join("del.txt").exists(),
            "deleted-in-worktree file must stay deleted"
        );
        assert_eq!(
            repo.read("renamed_new.txt").unwrap(),
            b"rename me\nmore\nunstaged tail\n",
            "renamed file content restored"
        );
        assert!(!repo.dir.join("renamed_old.txt").exists());
        assert_eq!(repo.read("untracked.txt").unwrap(), b"untracked content\n");
        assert_eq!(
            repo.read("untracked_dir/nested.txt").unwrap(),
            b"nested\n",
            "untracked dir restored"
        );
        assert_eq!(
            repo.read("binary.bin").unwrap(),
            vec![0u8, 159, 146, 150, 0, 1, 2, 255],
            "binary worktree content restored"
        );

        // The status must show the same change kinds as before the discard.
        let status = engine.status(&repo.path_str()).await.unwrap();
        let by_path: std::collections::HashMap<&str, &crate::core::engine::FileStatus> =
            status.iter().map(|f| (f.path.as_str(), f)).collect();
        assert!(by_path["staged.txt"].staged);
        let base = by_path["base.txt"];
        assert!(base.staged && base.unstaged);
        let ren = by_path["renamed_new.txt"];
        assert!(ren.staged && ren.orig_path.is_some());
        assert!(by_path["del.txt"].unstaged && !by_path["del.txt"].staged);
        assert!(by_path["untracked.txt"].untracked);
        assert!(by_path["binary.bin"].staged);
        // Index state really restored (staged content, not just worktree).
        let idx = engine
            .ls_index(
                &repo.path_str(),
                &["staged.txt".into(), "renamed_new.txt".into()],
            )
            .await
            .unwrap();
        assert_eq!(idx.len(), 2);

        // Snapshot is consumable: delete after restore, list shrinks.
        assert_eq!(recovery.list(&repo.dir).len(), 1);
        recovery.delete(&repo.dir, &snap.id).unwrap();
        assert!(recovery.list(&repo.dir).is_empty());
    }

    #[tokio::test]
    async fn restore_worktree_scope_keeps_staged() {
        // scope=worktree: discard the unstaged edit, staged state kept;
        // restore brings the worktree edit back without touching the index.
        let repo = TempRepo::new("worktree-scope");
        let engine = repo.engine();
        let recovery = mgr("worktree-scope");

        repo.write("a.txt", b"line1\n");
        git(&repo.dir, &["add", "-A"]);
        git(&repo.dir, &["commit", "-q", "-m", "init"]);
        repo.write("a.txt", b"line1\nline2 staged\n");
        git(&repo.dir, &["add", "a.txt"]);
        repo.write("a.txt", b"line1\nline2 staged\nline3 wt\n");

        let status = engine.status(&repo.path_str()).await.unwrap();
        let targets = repo.classify(&status, &["a.txt"]);
        let snap = recovery
            .snapshot_discard(&engine, &repo.dir, &targets, DiscardScope::Worktree)
            .await
            .unwrap();
        engine
            .restore_worktree(&repo.path_str(), &["a.txt".into()])
            .await
            .unwrap();
        assert_eq!(repo.read("a.txt").unwrap(), b"line1\nline2 staged\n");

        recovery
            .restore_discard(&engine, &repo.dir, &snap.id)
            .await
            .unwrap();
        assert_eq!(
            repo.read("a.txt").unwrap(),
            b"line1\nline2 staged\nline3 wt\n"
        );
        let status = engine.status(&repo.path_str()).await.unwrap();
        let f = &status[0];
        assert!(f.staged && f.unstaged);
    }

    #[tokio::test]
    async fn restore_conflict_state() {
        // Discarding an unmerged path (restore_to_head) then restoring the
        // snapshot brings back the full conflict (stages + worktree content).
        let repo = TempRepo::new("conflict");
        let engine = repo.engine();
        let recovery = mgr("conflict");

        repo.write("c.txt", b"base\n");
        git(&repo.dir, &["add", "-A"]);
        git(&repo.dir, &["commit", "-q", "-m", "init"]);
        git(&repo.dir, &["checkout", "-q", "-b", "side"]);
        repo.write("c.txt", b"side\n");
        git(&repo.dir, &["commit", "-q", "-am", "side"]);
        git(&repo.dir, &["checkout", "-q", "main"]);
        repo.write("c.txt", b"main\n");
        git(&repo.dir, &["commit", "-q", "-am", "main"]);
        // A conflicted merge exits 1 by design — don't use the asserting helper.
        let out = Command::new("git")
            .args(["-C", repo.dir.to_str().unwrap(), "merge", "side"])
            .output()
            .unwrap();
        assert_ne!(out.status.code(), Some(0));
        let status = engine.status(&repo.path_str()).await.unwrap();
        assert!(
            status.iter().any(|f| f.conflict),
            "expected a conflict: {status:?}"
        );

        let targets = repo.classify(&status, &["c.txt"]);
        let snap = recovery
            .snapshot_discard(&engine, &repo.dir, &targets, DiscardScope::All)
            .await
            .unwrap();
        engine
            .restore_to_head(&repo.path_str(), &["c.txt".into()])
            .await
            .unwrap();
        let status = engine.status(&repo.path_str()).await.unwrap();
        assert!(!status.iter().any(|f| f.conflict));

        recovery
            .restore_discard(&engine, &repo.dir, &snap.id)
            .await
            .unwrap();
        // The snapshot captured the conflicted worktree state verbatim.
        let content = repo.read("c.txt").unwrap();
        assert!(
            content.starts_with(b"<<<<<<<"),
            "conflict markers restored, got {content:?}"
        );
        let status = engine.status(&repo.path_str()).await.unwrap();
        let f = status.iter().find(|f| f.conflict).expect("conflict back");
        assert!(f.staged && f.unstaged);
        // All three unmerged stages are back in the index.
        let idx = engine
            .ls_index(&repo.path_str(), &["c.txt".into()])
            .await
            .unwrap();
        let mut stages: Vec<u32> = idx.iter().map(|e| e.stage).collect();
        stages.sort_unstable();
        assert_eq!(stages, vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn oversize_file_warns_but_restores_index() {
        let repo = TempRepo::new("oversize");
        let engine = repo.engine();
        let recovery = mgr("oversize");

        repo.write("big.txt", b"x\n");
        git(&repo.dir, &["add", "-A"]);
        git(&repo.dir, &["commit", "-q", "-m", "init"]);

        // A real oversized file (> cap): sparse-ish 10MB+1 write is cheap.
        let big = vec![b'x'; (MAX_COPY_BYTES + 1) as usize];
        repo.write("big.txt", &big);
        git(&repo.dir, &["add", "big.txt"]);

        let status = engine.status(&repo.path_str()).await.unwrap();
        let targets = repo.classify(&status, &["big.txt"]);
        let snap = recovery
            .snapshot_discard(&engine, &repo.dir, &targets, DiscardScope::All)
            .await
            .unwrap();

        let entries = recovery.list(&repo.dir);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].warnings.len(), 1, "over-cap warning recorded");
        assert_eq!(entries[0].file_count, 1);

        engine
            .restore_to_head(&repo.path_str(), &["big.txt".into()])
            .await
            .unwrap();
        let clean = engine.status(&repo.path_str()).await.unwrap();
        assert!(clean.is_empty(), "expected clean after discard: {clean:?}");

        // Restore: the staged state returns via the index blob; the worktree
        // copy was not kept (over cap, documented limitation) so the file
        // content stays at HEAD ("x\n"), which re-appears as a modification.
        recovery
            .restore_discard(&engine, &repo.dir, &snap.id)
            .await
            .unwrap();
        let idx = engine
            .ls_index(&repo.path_str(), &["big.txt".into()])
            .await
            .unwrap();
        assert_eq!(idx.len(), 1, "staged state restored from blob");
        assert_eq!(
            repo.read("big.txt").unwrap(),
            b"x\n",
            "over-cap content NOT restored (warning covers this)"
        );
        let status = engine.status(&repo.path_str()).await.unwrap();
        assert!(
            status.iter().any(|f| f.path == "big.txt" && f.unstaged),
            "index≠worktree difference visible again"
        );
    }

    #[tokio::test]
    async fn prune_removes_expired_snapshots() {
        let repo = TempRepo::new("prune");
        let engine = repo.engine();
        let recovery = mgr("prune");

        repo.write("a.txt", b"a\n");
        git(&repo.dir, &["add", "-A"]);
        git(&repo.dir, &["commit", "-q", "-m", "init"]);

        // A worktree modification to snapshot.
        repo.write("a.txt", b"a2\n");

        let status = engine.status(&repo.path_str()).await.unwrap();
        let targets = repo.classify(&status, &["a.txt"]);
        let snap = recovery
            .snapshot_discard(&engine, &repo.dir, &targets, DiscardScope::Worktree)
            .await
            .unwrap();

        // Backdate both the dir name (prune key) and the meta timestamp.
        let dir = recovery.snapshot_dir(&repo.dir, &snap.id);
        let old_dir = dir.parent().unwrap().join("1000-backdated");
        std::fs::rename(&dir, &old_dir).unwrap();
        let meta_path = old_dir.join("meta.json");
        let mut meta: DiscardMeta =
            serde_json::from_slice(&std::fs::read(&meta_path).unwrap()).unwrap();
        meta.created_at_ms = 1_000;
        std::fs::write(&meta_path, serde_json::to_vec(&meta).unwrap()).unwrap();

        recovery.prune_expired(&repo.dir);
        assert!(!old_dir.exists(), "expired snapshot pruned");
        assert!(recovery.list(&repo.dir).is_empty());
    }

    #[test]
    fn invalid_ids_rejected() {
        assert!(validate_id("1750000000000-123").is_ok());
        assert!(validate_id("../evil").is_err());
        assert!(validate_id("a/b").is_err());
        assert!(validate_id("").is_err());
        assert!(validate_id("abc$$").is_err());
    }
}
