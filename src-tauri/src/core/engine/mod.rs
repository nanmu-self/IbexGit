use crate::core::error::AppError;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub mod conflict;
pub mod parse;
pub mod patch;
pub mod stats;
pub mod untracked;

pub use crate::core::graph::{GraphEdge, GraphPage, GraphRow};

// =====================
// Types (shared)
// =====================

/// One path in the git index (from `git ls-files -s`), used by the
/// recovery snapshot to record the exact staged state (mode/sha/stage).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct IndexEntry {
    pub path: String,
    /// e.g. "100644", "100755", "160000", "120000"
    pub mode: String,
    /// 40-hex object id (blob, guaranteed present in the object DB).
    pub sha: String,
    /// 0 = merged, 1..3 = unmerged stages (base/ours/theirs).
    pub stage: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct FileStatus {
    pub path: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orig_path: Option<String>,
    pub submodule: bool,
    /// Submodule with modified content (porcelain v2 `sub = S`).
    pub submodule_dirty: bool,
    /// Submodule whose checked-in commit differs from the index (`sub = M`).
    pub submodule_commit_changed: bool,
    /// The worktree change consists only of line-ending differences
    /// (CRLF/LF); detected via the `--ignore-cr-at-eol` numstat diff.
    pub eol_only: bool,
    pub staged: bool,
    pub unstaged: bool,
    pub untracked: bool,
    /// skip-worktree (porcelain v2 XY contains `S`).
    pub skipped: bool,
    pub conflict: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct CommitInfo {
    pub hash: String,
    pub short_hash: String,
    pub author: String,
    pub email: String,
    pub date: String,
    pub message: String,
    pub refs: Vec<String>,
    pub parents: Vec<String>,
}

/// Search/filter for the history graph query (PLAN P5 搜索/筛选).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct GraphFilter {
    /// Message search (`--grep -i`). A pure-hex prefix of ≥ 4 chars is
    /// treated as a hash jump instead: it resolves via `rev-parse` and the
    /// graph starts at that commit.
    pub text: Option<String>,
    /// Author substring (`--author -i`).
    pub author: Option<String>,
    /// `--since` (git date, e.g. `2026-01-01`).
    pub since: Option<String>,
    /// `--until`.
    pub until: Option<String>,
    /// History touching any of these paths.
    pub paths: Vec<String>,
}

impl GraphFilter {
    /// `true` when nothing is set (the unfiltered graph).
    pub fn is_empty(&self) -> bool {
        self.text.is_none()
            && self.author.is_none()
            && self.since.is_none()
            && self.until.is_none()
            && self.paths.is_empty()
    }
}

/// One changed file of a commit (P5 提交详情变更列表).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct CommitFileStat {
    /// git name-status letter: `A` added, `M` modified, `D` deleted,
    /// `T` typechange, `R` renamed, `C` copied, `U` unmerged.
    pub status: String,
    /// Rename/copy similarity score (e.g. `93` for `R93`), if any.
    pub score: Option<u32>,
    /// Destination path (for renames/copies the new name).
    pub path: String,
    /// Source path for renames/copies.
    pub orig_path: Option<String>,
}

/// Commit metadata + changed files (P5 提交详情).
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct CommitDetail {
    pub commit: CommitInfo,
    /// First parent, `None` for a root commit (diff then uses the empty
    /// tree on the frontend side).
    pub parent: Option<String>,
    pub files: Vec<CommitFileStat>,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct DiffLine {
    pub content: String,
    pub left_no: Option<u32>,
    pub right_no: Option<u32>,
    pub kind: DiffLineKind,
    /// The line is followed by `\\ No newline at end of file`: the file on
    /// this line's side ends without a trailing newline. Only the genuine
    /// last line of a side can carry this; the patch builder uses it to
    /// reproduce correct `\\ No newline` markers (P4 行级暂存边界用例).
    #[serde(default)]
    pub no_eol: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum DiffLineKind {
    Context,
    Add,
    Remove,
    Header,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct DiffHunk {
    pub old_start: u32,
    pub old_count: u32,
    pub new_start: u32,
    pub new_count: u32,
    pub header: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct DiffFile {
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub similarity: Option<u32>,
    pub binary: bool,
    pub hunks: Vec<DiffHunk>,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct DiffModel {
    /// Identity of the cached model on the Rust side (PLAN P4: 行级操作由前端
    /// 上报 model id + 行号，Rust 从同一模型构造 patch，杜绝双端解析漂移).
    /// 0 = not cached (parse-time default).
    #[serde(default)]
    pub id: u32,
    pub source: DiffSource,
    pub old_revision: Option<String>,
    pub new_revision: Option<String>,
    pub files: Vec<DiffFile>,
}

impl DiffModel {
    /// Find a file by its displayed path (new path, falling back to old).
    pub fn find_file(&self, path: &str) -> Option<&DiffFile> {
        self.files
            .iter()
            .find(|f| f.new_path.as_deref() == Some(path) || f.old_path.as_deref() == Some(path))
    }
}

/// Options for [`GitEngine::diff`] (P4: context expansion + whitespace modes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct DiffOptions {
    /// `--unified=N` context lines (git default 3). The diff viewer expands
    /// collapsed context by re-fetching with a larger value.
    pub context_lines: u32,
    /// `--ignore-all-space` (`-w`): ignore whitespace when comparing lines.
    pub ignore_whitespace: bool,
}

impl Default for DiffOptions {
    fn default() -> Self {
        Self {
            context_lines: 3,
            ignore_whitespace: false,
        }
    }
}

/// One selected diff line for line-level operations (P4 行级暂存): indices
/// into the cached [`DiffModel`] the frontend is displaying.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct LineSelection {
    /// Index into `DiffFile.hunks`.
    pub hunk: u32,
    /// Index into `DiffHunk.lines`.
    pub line: u32,
}

/// Content of one file revision (worktree or a git object), base64-encoded
/// for the wire; `data` is `None` when the file exceeds the transfer cap.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct FileContent {
    /// Saturates at `u32::MAX` for absurd sizes (the UI only needs a
    /// "too large" signal past the transfer cap).
    pub size: u32,
    pub data: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum DiffSource {
    Worktree,
    Staged,
    Commit,
    Stash,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct BranchInfo {
    pub name: String,
    pub full_name: String,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub current: bool,
    pub detached: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct TagInfo {
    pub name: String,
    pub full_name: String,
    pub target: String,
    pub tagger: Option<String>,
    pub date: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct RemoteInfo {
    pub name: String,
    pub url: String,
    pub fetch_url: String,
    pub push_url: String,
}

/// One backup ref under `refs/ibexgit/backups/` (PLAN §4.7 轨道 B): a
/// recovery anchor created before dangerous branch/HEAD rewrites.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct BackupRef {
    /// Short name, e.g. `reset-1699999999999`.
    pub name: String,
    /// Full refname, e.g. `refs/ibexgit/backups/reset-1699999999999`.
    pub full_name: String,
    pub hash: String,
    pub short_hash: String,
    pub date: String,
    pub subject: String,
}

/// What the frontend needs to undo one reset (PLAN P6: 一键撤销).
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct ResetUndo {
    /// Backup ref created at the pre-reset HEAD (track B).
    pub backup_ref: String,
    /// Track-A snapshot id (hard resets only; restores index + worktree).
    pub snapshot_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct StashEntry {
    /// Position in the stash stack (`stash@{N}`); u32 on the wire.
    pub index: u32,
    pub message: String,
    pub branch: Option<String>,
    pub date: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct ReflogEntry {
    pub hash: String,
    pub short_hash: String,
    pub ref_name: String,
    pub message: String,
    pub date: String,
    pub author: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct RebaseState {
    pub state: String,
    pub current_step: u32,
    pub total_steps: u32,
    pub current_commit: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct MergeResult {
    pub success: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct PullResult {
    pub success: bool,
    pub message: String,
    pub fast_forward: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct CommitResult {
    pub hash: String,
    pub short_hash: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct CloneProgress {
    pub phase: String,
    pub percent: Option<f32>,
    pub message: String,
}

/// One commit of a single file's history (`git log --follow`, P9 文件追溯).
/// Entries are newest first; the path fields track the file across renames.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct FileCommit {
    pub hash: String,
    pub short_hash: String,
    pub author: String,
    pub email: String,
    pub date: String,
    pub message: String,
    pub parents: Vec<String>,
    /// The path this file had **at this commit** (for renames: the new path).
    pub path: String,
    /// Old path when this commit renamed/copied the file (`R`/`C` entry).
    pub orig_path: Option<String>,
    /// name-status letter of this file in the commit: A/M/D/T/R/C.
    pub status: String,
    /// Rename/copy similarity score (e.g. `93` for `R93`).
    pub score: Option<u32>,
}

/// One distinct commit in a blame result (P9 Blame 视图). Lines reference
/// commits by index to avoid repeating the metadata per line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct BlameCommit {
    pub hash: String,
    pub short_hash: String,
    pub author: String,
    pub email: String,
    /// Author date, ISO 8601 with the author-tz offset.
    pub date: String,
    pub summary: String,
    /// The path this file had at this commit (porcelain `filename` field —
    /// the pre-rename name for lines predating a rename).
    pub path: String,
    /// History ends here (root commit / shallow boundary).
    pub boundary: bool,
    /// Working-tree change that is not committed yet (all-zero sha).
    pub uncommitted: bool,
}

/// One blamed line; `commit` is an index into [`BlameResult::commits`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct BlameLine {
    pub commit: u32,
    /// Line number in the commit's version of the file (1-based).
    pub orig_no: u32,
    /// Line number in the current worktree file (1-based).
    pub final_no: u32,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, specta::Type)]
pub struct BlameResult {
    pub commits: Vec<BlameCommit>,
    pub lines: Vec<BlameLine>,
}

/// In-progress repository operation (P8 仓库状态头: merge/rebase/…).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    Merge,
    Rebase,
    CherryPick,
    Revert,
    /// `git am` residue (rebase-apply without rebase files).
    Apply,
    Bisect,
}

impl OperationKind {
    /// `continue`/`abort` subcommand family; `None` = no such verb.
    pub fn verb(self) -> Option<&'static str> {
        match self {
            OperationKind::Merge => Some("merge"),
            OperationKind::Rebase => Some("rebase"),
            OperationKind::CherryPick => Some("cherry-pick"),
            OperationKind::Revert => Some("revert"),
            OperationKind::Apply => Some("am"),
            OperationKind::Bisect => Some("bisect"),
        }
    }
}

/// What the frontend needs for the operation guidance banner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct OperationState {
    pub kind: OperationKind,
    /// Head being merged/rebased onto (full sha; frontend truncates).
    pub onto: Option<String>,
    /// 1-based step within the sequence (rebase/cherry-pick sequences).
    pub step: Option<u32>,
    pub total: Option<u32>,
    /// `MERGE_MSG` (merge/cherry-pick/revert) for the commit prefill.
    pub message: Option<String>,
}

// ---- P10: 配置查看器 / 提交模板 ----

/// One `key = value` pair from `git config --list -z` (P10 Git 配置查看器).
/// Valueless (boolean-true) keys get an empty value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct ConfigEntry {
    pub key: String,
    pub value: String,
}

/// The global gitignore file (`core.excludesFile` or the platform default
/// `~/.config/git/ignore`), read for the read-only viewer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct GitignoreFile {
    /// Resolved absolute path (display purpose).
    pub path: String,
    /// Raw file content (lossy UTF-8).
    pub content: String,
}

/// The commit message template (`commit.template`), resolved to an absolute
/// path and read (P10 提交辅助).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct CommitTemplate {
    pub path: String,
    pub content: String,
}

/// One common config key resolved for a repository (P10 仓库设置): the
/// repo-local value (`.git/config`) and the effective value git would use
/// (merged system + global + local). `local == None` 意味着继承上层作用域。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct RepoConfigValue {
    pub key: String,
    /// Value written in the repo-local config; `None` = no local override.
    pub local: Option<String>,
    /// Effective value after merging all scopes; `None` = not set anywhere.
    pub effective: Option<String>,
}

// ---- P11: AI 报告采集（log + numstat） ----

/// One file's line-count stats within a commit (`git log --numstat`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct NumstatFile {
    pub path: String,
    /// Pre-rename path (`git log --numstat -z` rename layout).
    pub orig_path: Option<String>,
    pub additions: u32,
    pub deletions: u32,
    pub binary: bool,
}

/// One commit with per-file stats — the AI report collector's unit.
/// 上下文只含 message/author/日期/numstat（ADR-013：默认不发 diff）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct NumstatCommit {
    pub hash: String,
    pub short_hash: String,
    pub author: String,
    pub email: String,
    /// Author date, ISO 8601 (`%aI`).
    pub date: String,
    pub subject: String,
    /// Source repo label (filled by the collector; empty from the engine).
    pub repo: String,
    pub files: Vec<NumstatFile>,
}

// =====================
// GitEngine trait
// =====================

#[async_trait]
pub trait GitEngine: Send + Sync {
    // Status
    async fn status(&self, repo: &str) -> Result<Vec<FileStatus>, AppError>;

    // Stage / Unstage
    async fn stage(&self, repo: &str, paths: &[String]) -> Result<(), AppError>;
    async fn unstage(&self, repo: &str, paths: &[String]) -> Result<(), AppError>;

    // Discard (worktree). Split into explicit primitives so the caller can
    // snapshot beforehand and orchestrate recovery (PLAN §4.7 轨道 A):
    // - restore_worktree: drop unstaged changes only (`git restore`)
    // - restore_to_head:  drop staged + unstaged (`git restore --source=HEAD
    //   --staged --worktree`); also the way to clear unmerged entries
    // - delete_untracked: physically remove untracked files (git does not
    //   know them; recovery snapshots must copy them beforehand)
    async fn restore_worktree(&self, repo: &str, paths: &[String]) -> Result<(), AppError>;
    async fn restore_to_head(&self, repo: &str, paths: &[String]) -> Result<(), AppError>;
    async fn delete_untracked(&self, repo: &str, paths: &[String]) -> Result<(), AppError>;

    /// Commit subject+body of HEAD (`git log -1 --format=%B`), or `None`
    /// when HEAD is unborn (fresh repository). Used by Amend.
    async fn head_message(&self, repo: &str) -> Result<Option<String>, AppError>;

    /// Append paths to `.gitignore` (anchored patterns, de-duplicated).
    async fn ignore_paths(&self, repo: &str, paths: &[String]) -> Result<(), AppError>;

    /// Index state for the given paths (`git ls-files -s -z`), including
    /// unmerged stages — the recovery snapshot's source of truth for
    /// restoring the exact staged state.
    async fn ls_index(&self, repo: &str, paths: &[String]) -> Result<Vec<IndexEntry>, AppError>;

    /// Write raw `--index-info` records (`<mode> <sha> <stage>\t<path>`,
    /// NUL-terminated) — restores snapshot index state, unmerged included.
    async fn update_index_info(&self, repo: &str, info: &str) -> Result<(), AppError>;

    /// Remove paths from the index (`git update-index --force-remove --stdin`).
    async fn remove_index_entries(&self, repo: &str, paths: &[String]) -> Result<(), AppError>;

    // Commit
    async fn commit(
        &self,
        repo: &str,
        message: &str,
        amend: bool,
        no_verify: bool,
    ) -> Result<CommitResult, AppError>;

    // Diff
    async fn diff(
        &self,
        repo: &str,
        source: DiffSource,
        rev_range: Option<(&str, &str)>,
        paths: &[String],
        opts: DiffOptions,
    ) -> Result<DiffModel, AppError>;

    /// Content of one file: `rev = None` reads the worktree file, otherwise
    /// `git cat-file blob <rev>:<path>`. Files above the transfer cap return
    /// `data = None` with the size filled in (image diff / P4 protection).
    async fn file_content(
        &self,
        repo: &str,
        path: &str,
        rev: Option<&str>,
    ) -> Result<FileContent, AppError>;

    // Log
    async fn log(
        &self,
        repo: &str,
        limit: u32,
        offset: u32,
        paths: &[String],
    ) -> Result<Vec<CommitInfo>, AppError>;

    /// One page of topo-ordered history for the commit graph (P5
    /// GraphQuery). `skip`/`limit` paginate within one refs generation;
    /// layout happens in `core::graph`, caching in `RepoManager`.
    async fn graph(
        &self,
        repo: &str,
        skip: u32,
        limit: u32,
        filter: &GraphFilter,
    ) -> Result<Vec<CommitInfo>, AppError>;

    /// Commit metadata + changed files (P5 提交详情). Merge commits diff
    /// against their first parent; root commits against the empty tree.
    async fn commit_detail(&self, repo: &str, hash: &str) -> Result<CommitDetail, AppError>;

    // History operations (P5)
    /// Cherry-pick the given commits onto HEAD, applying in list order.
    async fn cherry_pick(&self, repo: &str, hashes: &[String]) -> Result<(), AppError>;
    /// Revert the given commits (oldest-effect-first list order).
    async fn revert(&self, repo: &str, hashes: &[String]) -> Result<(), AppError>;
    /// Restore file(s) from a revision into the worktree only
    /// (`git restore --source=<rev> --worktree`); the index is untouched.
    async fn restore_from(
        &self,
        repo: &str,
        source: &str,
        paths: &[String],
    ) -> Result<(), AppError>;

    // Branch
    async fn list_branches(&self, repo: &str) -> Result<Vec<BranchInfo>, AppError>;
    async fn create_branch(
        &self,
        repo: &str,
        name: &str,
        start_point: Option<&str>,
    ) -> Result<(), AppError>;
    async fn delete_branch(&self, repo: &str, name: &str, force: bool) -> Result<(), AppError>;
    async fn rename_branch(
        &self,
        repo: &str,
        old_name: &str,
        new_name: &str,
    ) -> Result<(), AppError>;
    async fn checkout_branch(&self, repo: &str, name: &str) -> Result<(), AppError>;
    /// List remote-tracking branches (`refs/remotes/<remote>/<branch>`,
    /// `name` is the short form like `origin/main`); symbolic refs such as
    /// `refs/remotes/origin/HEAD` are excluded.
    async fn list_remote_branches(&self, repo: &str) -> Result<Vec<BranchInfo>, AppError>;
    /// Check out a remote-tracking branch as a local branch: reuses an
    /// existing local branch of the same name, otherwise creates one
    /// tracking `refs/remotes/<remote>/<branch>`.
    async fn checkout_remote_branch(
        &self,
        repo: &str,
        remote: &str,
        branch: &str,
    ) -> Result<(), AppError>;
    /// Track a remote branch (`git branch --set-upstream-to=<upstream>`);
    /// `None` removes the tracking relationship.
    async fn set_branch_upstream(
        &self,
        repo: &str,
        branch: &str,
        upstream: Option<&str>,
    ) -> Result<(), AppError>;

    // Tag
    async fn list_tags(&self, repo: &str) -> Result<Vec<TagInfo>, AppError>;
    async fn create_tag(
        &self,
        repo: &str,
        name: &str,
        message: Option<&str>,
        target: &str,
    ) -> Result<(), AppError>;
    async fn delete_tag(&self, repo: &str, name: &str) -> Result<(), AppError>;

    // Stash
    async fn list_stash(&self, repo: &str) -> Result<Vec<StashEntry>, AppError>;
    async fn stash_push(&self, repo: &str, message: Option<&str>) -> Result<usize, AppError>;
    /// Apply a stash without dropping it (`git stash apply`).
    async fn stash_apply(&self, repo: &str, index: usize) -> Result<(), AppError>;
    async fn stash_pop(&self, repo: &str, index: usize) -> Result<(), AppError>;
    async fn stash_drop(&self, repo: &str, index: usize) -> Result<(), AppError>;

    // Remote
    async fn list_remotes(&self, repo: &str) -> Result<Vec<RemoteInfo>, AppError>;
    async fn add_remote(&self, repo: &str, name: &str, url: &str) -> Result<(), AppError>;
    async fn remove_remote(&self, repo: &str, name: &str) -> Result<(), AppError>;
    /// Rewrite the fetch (default) or push URL of a remote.
    async fn set_remote_url(
        &self,
        repo: &str,
        name: &str,
        url: &str,
        push: bool,
    ) -> Result<(), AppError>;
    /// Drop stale remote-tracking refs (`git remote prune <name>`).
    async fn prune_remote(&self, repo: &str, name: &str) -> Result<(), AppError>;
    async fn fetch(&self, repo: &str, remote: Option<&str>) -> Result<(), AppError>;

    // Reflog
    async fn reflog(
        &self,
        repo: &str,
        ref_name: Option<&str>,
    ) -> Result<Vec<ReflogEntry>, AppError>;

    // Reset
    async fn reset(&self, repo: &str, mode: &str, target: &str) -> Result<(), AppError>;

    // Clean (P6: preview → per-item confirm → delete; never run blind)
    /// Untracked files/dirs `git clean -fd` would remove (ignored files are
    /// not included, matching clean without `-x`). Directories end with `/`.
    async fn clean_list(&self, repo: &str) -> Result<Vec<String>, AppError>;
    /// Remove the given untracked paths (`git clean -fd -- <paths>`).
    async fn clean(&self, repo: &str, paths: &[String]) -> Result<(), AppError>;

    // Branch compare / merge-rebase previews (P6)
    /// Best common ancestor of two revs; `None` when they share no history.
    async fn merge_base(&self, repo: &str, a: &str, b: &str) -> Result<Option<String>, AppError>;
    /// `(ahead, behind)` of `left` relative to `right` via
    /// `git rev-list --left-right --count left...right`.
    async fn range_count(
        &self,
        repo: &str,
        left: &str,
        right: &str,
    ) -> Result<(u32, u32), AppError>;
    /// Commits in a rev range (e.g. `HEAD..origin/main`), oldest not
    /// guaranteed — order follows `git log` (newest first).
    async fn rev_list(
        &self,
        repo: &str,
        range: &str,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<CommitInfo>, AppError>;

    // Backup refs (PLAN §4.7 轨道 B: RecoveryManager 的提交级备份锚点)
    /// Point an arbitrary ref at `target` (`git update-ref <name> <target>`).
    async fn update_ref(&self, repo: &str, name: &str, target: &str) -> Result<(), AppError>;
    /// All refs under `refs/ibexgit/backups/` (oldest first).
    async fn list_backup_refs(&self, repo: &str) -> Result<Vec<BackupRef>, AppError>;
    /// Delete the given backup refs (孤儿备份清理入口).
    async fn delete_backup_refs(&self, repo: &str, names: &[String]) -> Result<(), AppError>;

    // Apply (patch)
    async fn apply(
        &self,
        repo: &str,
        patch: &str,
        cached: bool,
        reverse: bool,
    ) -> Result<(), AppError>;

    // Push / Pull / Rebase / Merge
    /// Push `branch` to `remote`. `force_with_lease` is the safe rewrite
    /// guard (P6: 前端二次确认); `set_upstream` publishes tracking; `tags`
    /// additionally pushes tags (`git push <remote> --tags`, used alone when
    /// `branch` is empty).
    async fn push(
        &self,
        repo: &str,
        remote: &str,
        branch: &str,
        force_with_lease: bool,
        set_upstream: bool,
        tags: bool,
    ) -> Result<(), AppError>;
    /// Pull with an explicit mode: `None`/"merge" → default merge,
    /// `"rebase"` → `--rebase`, `"ff_only"` → `--ff-only` (P6 策略选择).
    async fn pull(
        &self,
        repo: &str,
        remote: Option<&str>,
        branch: Option<&str>,
        mode: Option<&str>,
    ) -> Result<PullResult, AppError>;
    async fn rebase(
        &self,
        repo: &str,
        target: &str,
        options: &[&str],
    ) -> Result<RebaseState, AppError>;
    /// Merge `target` into the current branch; `ff_only` refuses to create
    /// a merge commit (P6 dry-run 预览由前端 rev-list 提供).
    async fn merge(&self, repo: &str, target: &str, ff_only: bool)
        -> Result<MergeResult, AppError>;

    // Clone / Init (P7)
    /// Clone a remote repository (P7 克隆对话框)。Runs **without** `-C`
    /// (the target dir doesn't exist yet); `--progress` is implicit via
    /// streaming stderr → `on_line` per progress line. Cancel kills the
    /// whole process tree.
    async fn clone_repo(
        &self,
        opts: &CloneOptions,
        cancel: Option<&crate::core::runner::CancelToken>,
        on_line: Option<Arc<dyn Fn(String) + Send + Sync>>,
    ) -> Result<(), AppError>;
    /// `git init`（P7 新建仓库；default branch 由 `-c init.defaultBranch=main`
    /// 固定，README/.gitignore 模板与首个提交由命令层处理）。
    async fn init_repo(&self, path: &str) -> Result<(), AppError>;

    // File trace (P9)
    /// Single-file history with rename following (`git log --follow`),
    /// newest first. `start` is the last hash of the previous page (the
    /// next page is fetched from that commit and its duplicate dropped —
    /// `--skip` miscounts under `--follow`, and a cursor at the rename
    /// boundary must itself be in the traversal for the rename link to be
    /// re-detected). `limit == 0` means unlimited.
    async fn file_history(
        &self,
        repo: &str,
        path: &str,
        limit: u32,
        start: Option<&str>,
    ) -> Result<Vec<FileCommit>, AppError>;
    /// Blame the current worktree version of a path (`git blame --porcelain`).
    /// Uncommitted lines reference an `uncommitted` pseudo-commit.
    async fn blame(&self, repo: &str, path: &str) -> Result<BlameResult, AppError>;

    // =====================
    // Conflicts & operation state (P8)
    // =====================
    /// All conflicted paths with lightweight classification (conflict file
    /// list: unmerged status, block counts, type badges).
    async fn conflict_list(&self, repo: &str) -> Result<Vec<conflict::ConflictSummary>, AppError>;
    /// Full model for one conflicted path (editor input).
    async fn conflict_model(
        &self,
        repo: &str,
        path: &str,
    ) -> Result<conflict::ConflictModel, AppError>;
    /// Write the resolved document back and stage the path (`git add`).
    /// Rejects text that still contains conflict markers.
    async fn resolve_conflict_text(
        &self,
        repo: &str,
        path: &str,
        text: &str,
    ) -> Result<(), AppError>;
    /// Keep one side of a conflict in the worktree and stage it
    /// (`side`: `ours` = stage 2, `theirs` = stage 3; sha-based blob write).
    async fn resolve_conflict_keep(
        &self,
        repo: &str,
        path: &str,
        side: &str,
    ) -> Result<(), AppError>;
    /// Drop the conflicted path (`git rm -f`); for DirectoryFile conflicts
    /// only the index entry is removed (`--cached`, the directory stays).
    async fn resolve_conflict_delete(&self, repo: &str, path: &str) -> Result<(), AppError>;
    /// Detect the in-progress operation (MERGE_HEAD / rebase-merge / …).
    async fn operation_state(&self, repo: &str) -> Result<Option<OperationState>, AppError>;
    /// Abort the in-progress operation (`merge --abort`, `rebase --abort`, …).
    async fn operation_abort(&self, repo: &str) -> Result<(), AppError>;
    /// Continue the in-progress operation after resolutions are staged.
    async fn operation_continue(&self, repo: &str) -> Result<(), AppError>;
    /// Skip the current commit of a rebase/cherry-pick/revert sequence.
    async fn operation_skip(&self, repo: &str) -> Result<(), AppError>;
    /// Open an external merge tool for one path (`git mergetool` with the
    /// tool configured via `-c`, prompts disabled, exit code trusted).
    async fn mergetool(
        &self,
        repo: &str,
        path: &str,
        tool: Option<&str>,
        cmd: Option<&str>,
    ) -> Result<(), AppError>;

    // ---- P10: 配置查看器 / 提交模板（只读） ----

    /// List the user-level config (`git config --list -z --global`).
    async fn config_global(&self) -> Result<Vec<ConfigEntry>, AppError>;
    /// List the repository-level config (`git config --list -z --local`).
    async fn config_local(&self, repo: &str) -> Result<Vec<ConfigEntry>, AppError>;
    /// List the effective (system + global + local) config for a repository
    /// (`git config --list -z`, no scope flag = merged).
    async fn config_merged(&self, repo: &str) -> Result<Vec<ConfigEntry>, AppError>;
    /// Set a user-level config key (`git config --global -- key value`);
    /// `None` unsets it (unsetting a missing key is not an error). The key
    /// is validated (`parse::config_key_valid`) before reaching argv.
    async fn config_set_global(&self, key: &str, value: Option<&str>) -> Result<(), AppError>;
    /// Set/unset a repository-local config key (`git config --local ...`);
    /// `None` unsets it. `--replace-all`/`--unset-all` 语义：同名键多行时
    /// 全量替换/移除，unset 不存在的键视为幂等成功。Key is validated
    /// (`parse::config_key_valid`) before reaching argv.
    async fn config_set_local(
        &self,
        repo: &str,
        key: &str,
        value: Option<&str>,
    ) -> Result<(), AppError>;
    /// Read the global gitignore (`core.excludesFile`, else the default
    /// `~/.config/git/ignore`); `None` when neither file exists.
    async fn global_gitignore(&self) -> Result<Option<GitignoreFile>, AppError>;
    /// Read the commit message template (`commit.template`, merged config;
    /// relative paths resolve against the repo root, `~` is expanded).
    async fn commit_template(&self, repo: &str) -> Result<Option<CommitTemplate>, AppError>;

    // ---- P11: AI 报告采集（Log 家族变体，只读） ----

    /// Commits with per-file line stats for the AI report collector
    /// (`git log --all --numstat -z` with `--since/--until/--author`).
    /// `--all` covers work on any branch; merge commits carry metadata but
    /// no stats. Newest first.
    async fn log_numstat(
        &self,
        repo: &str,
        since: Option<&str>,
        until: Option<&str>,
        author: Option<&str>,
        limit: u32,
    ) -> Result<Vec<NumstatCommit>, AppError>;

    // ---- 提交统计（只读，stats 对话框） ----

    /// Per-branch commit statistics (`git log --no-merges`, one call, four
    /// bucket sets: all-time by month / this month by day / this week by
    /// day / today by hour, all in the local timezone) + per-email
    /// contributor counts. Aggregation lives in [`stats::aggregate_stats`]
    /// with an injected `Local::now()`.
    async fn commit_stats(&self, repo: &str, rev: &str) -> Result<CommitStatsDto, AppError>;
}

pub mod cli;
pub use cli::CliEngine;
pub use stats::{CommitStatsDto, ContributorStat, StatBucket};

/// Clone 参数（P7 克隆对话框）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct CloneOptions {
    pub url: String,
    /// 目标目录（绝对路径）。
    pub dest: String,
    /// `--depth N`（浅克隆）。
    pub depth: Option<u32>,
    /// `--single-branch`。
    pub single_branch: bool,
    /// `--recurse-submodules`。
    pub recurse_submodules: bool,
}
