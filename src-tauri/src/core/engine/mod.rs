use crate::core::error::AppError;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

pub mod parse;

// =====================
// Types (shared)
// =====================

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileStatus {
    pub path: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orig_path: Option<String>,
    pub submodule: bool,
    pub staged: bool,
    pub unstaged: bool,
    pub untracked: bool,
    pub skipped: bool,
    pub conflict: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffLine {
    pub content: String,
    pub left_no: Option<u32>,
    pub right_no: Option<u32>,
    pub kind: DiffLineKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffLineKind {
    Context,
    Add,
    Remove,
    Header,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffHunk {
    pub old_start: u32,
    pub old_count: u32,
    pub new_start: u32,
    pub new_count: u32,
    pub header: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffFile {
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub similarity: Option<u32>,
    pub binary: bool,
    pub hunks: Vec<DiffHunk>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffModel {
    pub source: DiffSource,
    pub old_revision: Option<String>,
    pub new_revision: Option<String>,
    pub files: Vec<DiffFile>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffSource {
    Worktree,
    Staged,
    Commit,
    Stash,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchInfo {
    pub name: String,
    pub full_name: String,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub current: bool,
    pub detached: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagInfo {
    pub name: String,
    pub full_name: String,
    pub target: String,
    pub tagger: Option<String>,
    pub date: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteInfo {
    pub name: String,
    pub url: String,
    pub fetch_url: String,
    pub push_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StashEntry {
    pub index: usize,
    pub message: String,
    pub branch: Option<String>,
    pub date: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReflogEntry {
    pub hash: String,
    pub short_hash: String,
    pub ref_name: String,
    pub message: String,
    pub date: String,
    pub author: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RebaseState {
    pub state: String,
    pub current_step: u32,
    pub total_steps: u32,
    pub current_commit: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeResult {
    pub success: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullResult {
    pub success: bool,
    pub message: String,
    pub fast_forward: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitResult {
    pub hash: String,
    pub short_hash: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloneProgress {
    pub phase: String,
    pub percent: Option<f32>,
    pub message: String,
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

    // Discard (worktree)
    async fn discard(&self, repo: &str, paths: &[String]) -> Result<(), AppError>;

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
    ) -> Result<DiffModel, AppError>;

    // Log
    async fn log(
        &self,
        repo: &str,
        limit: u32,
        offset: u32,
        paths: &[String],
    ) -> Result<Vec<CommitInfo>, AppError>;

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
    async fn stash_pop(&self, repo: &str, index: usize) -> Result<(), AppError>;
    async fn stash_drop(&self, repo: &str, index: usize) -> Result<(), AppError>;

    // Remote
    async fn list_remotes(&self, repo: &str) -> Result<Vec<RemoteInfo>, AppError>;
    async fn fetch(&self, repo: &str, remote: Option<&str>) -> Result<(), AppError>;

    // Reflog
    async fn reflog(
        &self,
        repo: &str,
        ref_name: Option<&str>,
    ) -> Result<Vec<ReflogEntry>, AppError>;

    // Reset
    async fn reset(&self, repo: &str, mode: &str, target: &str) -> Result<(), AppError>;

    // Apply (patch)
    async fn apply(
        &self,
        repo: &str,
        patch: &str,
        cached: bool,
        reverse: bool,
    ) -> Result<(), AppError>;

    // Push / Pull / Rebase / Merge
    async fn push(
        &self,
        repo: &str,
        remote: &str,
        branch: &str,
        force_with_lease: bool,
        set_upstream: bool,
    ) -> Result<(), AppError>;
    async fn pull(
        &self,
        repo: &str,
        remote: Option<&str>,
        branch: Option<&str>,
        strategy: Option<&str>,
    ) -> Result<PullResult, AppError>;
    async fn rebase(
        &self,
        repo: &str,
        target: &str,
        options: &[&str],
    ) -> Result<RebaseState, AppError>;
    async fn merge(
        &self,
        repo: &str,
        target: &str,
        strategy: Option<&str>,
    ) -> Result<MergeResult, AppError>;

    // Blame (P9)
    async fn blame(&self, repo: &str, path: &str) -> Result<Vec<ReflogEntry>, AppError>;
}

pub mod cli;
pub use cli::CliEngine;
