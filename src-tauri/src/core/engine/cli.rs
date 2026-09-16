use crate::core::engine::{
    self, parse, CloneOptions, DiffModel, DiffOptions, DiffSource, FileContent,
};
use crate::core::error::AppError;
use crate::core::runner::{CancelToken, GitProcessRunner, ProcessResult, StdinMode};
use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

/// 网络操作（fetch/pull/push/clone）失败时：stderr 含取消标记 →
/// `CredentialCancelled`（用户在凭据框点了取消，而非认证失败；设计文档 §5）。
fn ensure_success_net(res: &ProcessResult) -> Result<(), AppError> {
    if res.exit_code != Some(0)
        && res
            .stderr
            .contains(crate::core::credential::CREDENTIAL_CANCELLED_MARKER)
    {
        return Err(AppError::CredentialCancelled);
    }
    match res.exit_code {
        Some(0) => Ok(()),
        Some(code) => Err(AppError::git_command(
            format!("git exited with code {}", code),
            res.stderr.clone(),
            res.stdout.clone(),
        )),
        None => Err(AppError::internal("git process terminated by signal")),
    }
}

/// Log format shared by `log`, `graph` and `commit_detail` (8 lines per
/// commit, see `parse_log`).
const LOG_FORMAT: &str = "%H%n%h%n%an%n%ae%n%aI%n%s%n%d%n%P";

pub struct CliEngine {
    runner: GitProcessRunner,
    git_path: String,
}

impl CliEngine {
    pub fn new(runner: GitProcessRunner, git_path: impl Into<String>) -> Self {
        Self {
            runner,
            git_path: git_path.into(),
        }
    }

    /// Flag worktree-modified entries whose only difference is CRLF/LF
    /// (PLAN P3: 行尾变更展示)：a path listed by plain `diff --numstat -z`
    /// but absent from the `--ignore-cr-at-eol` variant is EOL-only.
    async fn mark_eol_only(
        &self,
        repo: &str,
        status: &mut [engine::FileStatus],
    ) -> Result<(), AppError> {
        let candidates: Vec<String> = status
            .iter()
            .filter(|f| f.unstaged && !f.untracked && !f.conflict && !f.submodule)
            .map(|f| f.path.clone())
            .collect();
        if candidates.is_empty() {
            return Ok(());
        }
        let normal = self.numstat_paths(repo, false).await?;
        if normal.is_empty() {
            return Ok(());
        }
        let ignore_cr = self.numstat_paths(repo, true).await?;
        let ignore_set: HashSet<&str> = ignore_cr.iter().map(|s| s.as_str()).collect();
        let eol_only: HashSet<&str> = normal
            .iter()
            .map(|s| s.as_str())
            .filter(|p| !ignore_set.contains(p))
            .collect();
        for f in status.iter_mut() {
            if eol_only.contains(f.path.as_str()) {
                f.eol_only = true;
            }
        }
        Ok(())
    }

    async fn numstat_paths(&self, repo: &str, ignore_cr: bool) -> Result<Vec<String>, AppError> {
        let args: &[&str] = if ignore_cr {
            &["-C", repo, "diff", "--numstat", "-z", "--ignore-cr-at-eol"]
        } else {
            &["-C", repo, "diff", "--numstat", "-z"]
        };
        let res = self.run(args, StdinMode::Null, None, None).await?;
        self.ensure_success(&res)?;
        Ok(parse::parse_numstat_paths(&res.stdout))
    }

    async fn run(
        &self,
        args: impl IntoIterator<Item = impl AsRef<str>>,
        stdin_mode: StdinMode,
        stdin_bytes: Option<&[u8]>,
        timeout_secs: Option<u64>,
    ) -> Result<ProcessResult, AppError> {
        let args_owned: Vec<String> = args.into_iter().map(|s| s.as_ref().to_string()).collect();
        let args_refs: Vec<&str> = args_owned.iter().map(|s| s.as_str()).collect();
        self.runner
            .run(
                &self.git_path,
                &args_refs,
                stdin_mode,
                stdin_bytes,
                timeout_secs,
            )
            .await
    }

    fn ensure_success(&self, res: &ProcessResult) -> Result<(), AppError> {
        match res.exit_code {
            Some(0) => Ok(()),
            Some(code) => Err(AppError::git_command(
                format!("git exited with code {}", code),
                res.stderr.clone(),
                res.stdout.clone(),
            )),
            None => Err(AppError::internal("git process terminated by signal")),
        }
    }
}

#[async_trait::async_trait]
impl engine::GitEngine for CliEngine {
    async fn status(&self, repo: &str) -> Result<Vec<engine::FileStatus>, AppError> {
        let args = ["-C", repo, "status", "--porcelain=v2", "-z"];
        let res = self.run(args, StdinMode::Null, None, None).await?;
        self.ensure_success(&res)?;
        let mut status = parse::parse_status(&res.stdout);
        self.mark_eol_only(repo, &mut status).await?;
        Ok(status)
    }

    async fn stage(&self, repo: &str, paths: &[String]) -> Result<(), AppError> {
        if paths.is_empty() {
            return Ok(());
        }
        let mut args = vec!["-C", repo, "add"];
        args.extend(paths.iter().map(|s| s.as_str()));
        let res = self.run(args, StdinMode::Null, None, None).await?;
        self.ensure_success(&res)
    }

    async fn unstage(&self, repo: &str, paths: &[String]) -> Result<(), AppError> {
        if paths.is_empty() {
            return Ok(());
        }
        let mut args = vec!["-C", repo, "restore", "--staged"];
        args.extend(paths.iter().map(|s| s.as_str()));
        let res = self.run(args, StdinMode::Null, None, None).await?;
        self.ensure_success(&res)
    }

    async fn restore_worktree(&self, repo: &str, paths: &[String]) -> Result<(), AppError> {
        if paths.is_empty() {
            return Ok(());
        }
        let mut args = vec!["-C", repo, "restore"];
        args.extend(paths.iter().map(|s| s.as_str()));
        let res = self.run(args, StdinMode::Null, None, None).await?;
        self.ensure_success(&res)
    }

    async fn restore_to_head(&self, repo: &str, paths: &[String]) -> Result<(), AppError> {
        if paths.is_empty() {
            return Ok(());
        }
        // `--source=HEAD -S -W` drops staged + unstaged in one go and also
        // clears unmerged entries (the documented conflict-reset route).
        let mut args = vec![
            "-C",
            repo,
            "restore",
            "--source=HEAD",
            "--staged",
            "--worktree",
        ];
        args.extend(paths.iter().map(|s| s.as_str()));
        let res = self.run(args, StdinMode::Null, None, None).await?;
        self.ensure_success(&res)
    }

    async fn delete_untracked(&self, repo: &str, paths: &[String]) -> Result<(), AppError> {
        for path in paths {
            let target = safe_join(repo, path)?;
            if target.is_file() {
                std::fs::remove_file(&target)?;
            } else if target.is_dir() {
                std::fs::remove_dir_all(&target)?;
            }
        }
        Ok(())
    }

    async fn head_message(&self, repo: &str) -> Result<Option<String>, AppError> {
        // Unborn HEAD → no message to amend.
        let verify = self
            .run(
                ["-C", repo, "rev-parse", "--verify", "-q", "HEAD"],
                StdinMode::Null,
                None,
                None,
            )
            .await?;
        if verify.exit_code != Some(0) {
            return Ok(None);
        }
        let res = self
            .run(
                ["-C", repo, "log", "-1", "--format=%B"],
                StdinMode::Null,
                None,
                None,
            )
            .await?;
        self.ensure_success(&res)?;
        Ok(Some(res.stdout.trim_end().to_string()))
    }

    async fn ignore_paths(&self, repo: &str, paths: &[String]) -> Result<(), AppError> {
        if paths.is_empty() {
            return Ok(());
        }
        let gitignore = Path::new(repo).join(".gitignore");
        let existing = std::fs::read_to_string(&gitignore).unwrap_or_default();
        let known: HashSet<String> = existing.lines().map(|l| l.trim().to_string()).collect();
        let mut out = String::new();
        if !existing.is_empty() && !existing.ends_with('\n') {
            out.push('\n');
        }
        let mut added = 0;
        for path in paths {
            // Anchored pattern: matches only at the repository root, avoiding
            // accidental matches of same-named files in nested directories.
            let pattern = format!("/{}", path.replace('\\', "/"));
            if known.contains(&pattern) || known.contains(path.as_str()) {
                continue;
            }
            out.push_str(&pattern);
            out.push('\n');
            added += 1;
        }
        if added == 0 {
            return Ok(());
        }
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&gitignore)?;
        file.write_all(out.as_bytes())?;
        Ok(())
    }

    async fn ls_index(
        &self,
        repo: &str,
        paths: &[String],
    ) -> Result<Vec<engine::IndexEntry>, AppError> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        let mut args = vec!["-C", repo, "ls-files", "-s", "-z", "--"];
        args.extend(paths.iter().map(|s| s.as_str()));
        let res = self.run(args, StdinMode::Null, None, None).await?;
        self.ensure_success(&res)?;
        Ok(parse::parse_ls_files(&res.stdout))
    }

    async fn update_index_info(&self, repo: &str, info: &str) -> Result<(), AppError> {
        if info.is_empty() {
            return Ok(());
        }
        let args = ["-C", repo, "update-index", "-z", "--index-info"];
        let res = self
            .run(args, StdinMode::Feed, Some(info.as_bytes()), None)
            .await?;
        self.ensure_success(&res)
    }

    async fn remove_index_entries(&self, repo: &str, paths: &[String]) -> Result<(), AppError> {
        if paths.is_empty() {
            return Ok(());
        }
        let args = [
            "-C",
            repo,
            "update-index",
            "-z",
            "--force-remove",
            "--stdin",
        ];
        let mut feed = String::new();
        for p in paths {
            feed.push_str(p);
            feed.push('\0');
        }
        let res = self
            .run(args, StdinMode::Feed, Some(feed.as_bytes()), None)
            .await?;
        self.ensure_success(&res)
    }

    async fn commit(
        &self,
        repo: &str,
        message: &str,
        amend: bool,
        no_verify: bool,
    ) -> Result<engine::CommitResult, AppError> {
        let mut args = vec!["-C", repo, "commit"];
        if amend {
            args.push("--amend");
        }
        if no_verify {
            args.push("--no-verify");
        }
        args.push("-m");
        args.push(message);

        let res = self.run(args, StdinMode::Null, None, None).await?;
        self.ensure_success(&res)?;

        // The commit's stdout only carries an abbreviated, decorated hash —
        // resolve the authoritative full hash via rev-parse.
        let res = self
            .run(
                ["-C", repo, "rev-parse", "HEAD"],
                StdinMode::Null,
                None,
                None,
            )
            .await?;
        self.ensure_success(&res)?;
        let hash = res.stdout.trim().to_string();
        Ok(engine::CommitResult {
            short_hash: hash.chars().take(7).collect(),
            hash,
            message: message.to_string(),
        })
    }

    async fn diff(
        &self,
        repo: &str,
        source: DiffSource,
        rev_range: Option<(&str, &str)>,
        paths: &[String],
        opts: DiffOptions,
    ) -> Result<DiffModel, AppError> {
        let mut args = vec![
            "-C".to_string(),
            repo.to_string(),
            "diff".to_string(),
            "--no-color".to_string(),
            "-M".to_string(),
            "--diff-filter=ACDMRTUXB".to_string(),
        ];
        if opts.context_lines != 3 {
            args.push(format!("--unified={}", opts.context_lines));
        }
        if opts.ignore_whitespace {
            args.push("-w".to_string());
        }
        match source {
            DiffSource::Staged => args.push("--cached".to_string()),
            DiffSource::Commit => {
                if let Some((a, b)) = rev_range {
                    args.push(a.to_string());
                    args.push(b.to_string());
                }
            }
            // Stash content diff: `stash@{N}^..stash@{N}` when the caller
            // selects an entry, otherwise the whole-latest `git diff stash`.
            DiffSource::Stash => {
                if let Some((a, b)) = rev_range {
                    args.push(a.to_string());
                    args.push(b.to_string());
                } else {
                    args.push("stash".to_string());
                }
            }
            DiffSource::Worktree => {}
        }
        if !paths.is_empty() {
            args.push("--".to_string());
            for p in paths {
                args.push(p.clone());
            }
        }

        let args_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let res = self.run(args_refs, StdinMode::Null, None, None).await?;
        let model = parse::parse_diff(&res.stdout, source, rev_range)?;
        Ok(model)
    }

    async fn file_content(
        &self,
        repo: &str,
        path: &str,
        rev: Option<&str>,
    ) -> Result<FileContent, AppError> {
        const MAX_CONTENT_BYTES: u64 = 10 * 1024 * 1024; // PLAN P4: >10MB 提示跳过
        match rev {
            None => {
                // Worktree file: size-check first, then read.
                let abs = safe_join(repo, path)?;
                let size = std::fs::metadata(&abs).map(|m| m.len()).unwrap_or(0);
                if size > MAX_CONTENT_BYTES {
                    return Ok(FileContent {
                        size: size.min(u32::MAX as u64) as u32,
                        data: None,
                    });
                }
                let bytes = std::fs::read(&abs)?;
                use base64::Engine as _;
                Ok(FileContent {
                    size: bytes.len() as u32,
                    data: Some(base64::engine::general_purpose::STANDARD.encode(bytes)),
                })
            }
            Some(rev) => {
                // Object revision: `git cat-file -s <rev>:<path>` then blob.
                let spec = format!("{rev}:{path}");
                let size_res = self
                    .run(
                        ["-C", repo, "cat-file", "-s", &spec],
                        StdinMode::Null,
                        None,
                        None,
                    )
                    .await?;
                self.ensure_success(&size_res)?;
                let size: u64 = size_res.stdout.trim().parse().unwrap_or(0);
                if size > MAX_CONTENT_BYTES {
                    return Ok(FileContent {
                        size: size.min(u32::MAX as u64) as u32,
                        data: None,
                    });
                }
                let res = self
                    .runner
                    .run_raw(
                        &self.git_path,
                        &["-C", repo, "cat-file", "blob", &spec],
                        StdinMode::Null,
                        None,
                        None,
                        None,
                    )
                    .await?;
                if res.exit_code != Some(0) {
                    return Err(AppError::git_command(
                        format!("git cat-file blob {spec}"),
                        res.stderr,
                        String::new(),
                    ));
                }
                use base64::Engine as _;
                Ok(FileContent {
                    size: res.stdout.len() as u32,
                    data: Some(base64::engine::general_purpose::STANDARD.encode(res.stdout)),
                })
            }
        }
    }

    async fn log(
        &self,
        repo: &str,
        limit: u32,
        offset: u32,
        paths: &[String],
    ) -> Result<Vec<engine::CommitInfo>, AppError> {
        let limit_s = limit.to_string();
        let offset_s = offset.to_string();
        let mut args = vec![
            "-C",
            repo,
            "log",
            "--topo-order",
            "--format=%H%n%h%n%an%n%ae%n%aI%n%s%n%d%n%P",
        ];
        args.push("--no-color");
        if limit > 0 {
            args.push("-n");
            args.push(&limit_s);
        }
        if offset > 0 {
            args.push("--skip");
            args.push(&offset_s);
        }
        if !paths.is_empty() {
            args.push("--");
            for p in paths {
                args.push(p.as_str());
            }
        }

        let args_refs: &[&str] = &args;
        let res = self.run(args_refs, StdinMode::Null, None, None).await?;
        let commits = parse::parse_log(&res.stdout);
        Ok(commits)
    }

    async fn graph(
        &self,
        repo: &str,
        skip: u32,
        limit: u32,
        filter: &engine::GraphFilter,
    ) -> Result<Vec<engine::CommitInfo>, AppError> {
        // Hash-jump: a pure-hex text of ≥4 chars resolves to a starting
        // commit; on failure it degrades to a message grep below.
        let mut start: Option<String> = None;
        if let Some(t) = filter.text.as_deref() {
            if t.len() >= 4 && t.chars().all(|c| c.is_ascii_hexdigit()) {
                let probe = format!("{t}^{{commit}}");
                if let Ok(res) = self
                    .run(
                        ["-C", repo, "rev-parse", "--verify", "--quiet", &probe],
                        StdinMode::Null,
                        None,
                        None,
                    )
                    .await
                {
                    if res.exit_code == Some(0) {
                        start = Some(res.stdout.trim().to_string());
                    }
                }
            }
        }

        let mut args: Vec<String> = vec![
            "-C".into(),
            repo.into(),
            "log".into(),
            "--topo-order".into(),
            "--no-color".into(),
            format!("--format={}", LOG_FORMAT),
        ];
        if let Some(s) = &start {
            args.push(s.clone());
        }
        if let Some(t) = filter.text.as_deref() {
            if start.is_none() && !t.is_empty() {
                args.push("-i".into());
                args.push("--grep".into());
                args.push(t.into());
            }
        }
        if let Some(a) = filter.author.as_deref() {
            if !a.is_empty() {
                args.push("-i".into());
                args.push("--author".into());
                args.push(a.into());
            }
        }
        if let Some(s) = filter.since.as_deref() {
            if !s.is_empty() {
                args.push("--since".into());
                args.push(s.into());
            }
        }
        if let Some(u) = filter.until.as_deref() {
            if !u.is_empty() {
                args.push("--until".into());
                args.push(u.into());
            }
        }
        if skip > 0 {
            args.push("--skip".into());
            args.push(skip.to_string());
        }
        if limit > 0 && limit != u32::MAX {
            args.push("-n".into());
            args.push(limit.to_string());
        }
        if !filter.paths.is_empty() {
            args.push("--".into());
            args.extend(filter.paths.iter().cloned());
        }

        let args_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let res = self.run(args_refs, StdinMode::Null, None, None).await?;
        if res.exit_code != Some(0) {
            // Unborn HEAD (fresh repository): an empty graph, not an error.
            if res.stderr.contains("does not have any commits") {
                return Ok(Vec::new());
            }
            return Err(AppError::git_command(
                format!("git exited with code {:?}", res.exit_code),
                res.stderr,
                res.stdout,
            ));
        }
        Ok(parse::parse_log(&res.stdout))
    }

    async fn commit_detail(
        &self,
        repo: &str,
        hash: &str,
    ) -> Result<engine::CommitDetail, AppError> {
        // 1. Commit metadata (same 8-line format as log).
        let res = self
            .run(
                [
                    "-C",
                    repo,
                    "log",
                    "-1",
                    "--no-color",
                    &format!("--format={}", LOG_FORMAT),
                    hash,
                ],
                StdinMode::Null,
                None,
                None,
            )
            .await?;
        self.ensure_success(&res)?;
        let mut commits = parse::parse_log(&res.stdout);
        let commit = commits
            .pop()
            .ok_or_else(|| AppError::parse(format!("no commit {hash:?}")))?;

        // 2. Changed files. Merge commits diff against the first parent;
        // root commits need `--root` (diff against the empty tree).
        let diff_args: Vec<&str> = if commit.parents.len() > 1 {
            vec![
                "-C",
                repo,
                "diff-tree",
                "-r",
                "-M",
                "-z",
                "--name-status",
                "--no-commit-id",
                &commit.parents[0],
                &commit.hash,
            ]
        } else {
            vec![
                "-C",
                repo,
                "diff-tree",
                "--root",
                "-r",
                "-M",
                "-z",
                "--name-status",
                "--no-commit-id",
                &commit.hash,
            ]
        };
        let res = self.run(diff_args, StdinMode::Null, None, None).await?;
        self.ensure_success(&res)?;
        let files = parse::parse_commit_files(&res.stdout);
        Ok(engine::CommitDetail {
            parent: commit.parents.first().cloned(),
            commit,
            files,
        })
    }

    async fn cherry_pick(&self, repo: &str, hashes: &[String]) -> Result<(), AppError> {
        let mut args = vec!["-C", repo, "cherry-pick"];
        args.extend(hashes.iter().map(|s| s.as_str()));
        let res = self.run(args, StdinMode::Null, None, None).await?;
        self.ensure_success(&res)
    }

    async fn revert(&self, repo: &str, hashes: &[String]) -> Result<(), AppError> {
        let mut args = vec!["-C", repo, "revert", "--no-edit"];
        args.extend(hashes.iter().map(|s| s.as_str()));
        let res = self.run(args, StdinMode::Null, None, None).await?;
        self.ensure_success(&res)
    }

    async fn restore_from(
        &self,
        repo: &str,
        source: &str,
        paths: &[String],
    ) -> Result<(), AppError> {
        // Worktree-only restore: the index is untouched.
        let mut args = vec![
            "-C",
            repo,
            "restore",
            "--source",
            source,
            "--worktree",
            "--",
        ];
        args.extend(paths.iter().map(|s| s.as_str()));
        let res = self.run(args, StdinMode::Null, None, None).await?;
        self.ensure_success(&res)
    }

    async fn list_branches(&self, repo: &str) -> Result<Vec<engine::BranchInfo>, AppError> {
        // for-each-ref gives upstream tracking (ahead/behind) that `branch -v` lacks.
        let args = [
            "-C",
            repo,
            "for-each-ref",
            "--format=%(refname)%00%(refname:short)%00%(upstream:short)%00%(upstream:track)%00%(HEAD)%00%(objectname)",
            "refs/heads/",
        ];
        let res = self.run(args, StdinMode::Null, None, None).await?;
        let branches = parse::parse_branches(&res.stdout);
        Ok(branches)
    }

    async fn create_branch(
        &self,
        repo: &str,
        name: &str,
        start_point: Option<&str>,
    ) -> Result<(), AppError> {
        let mut args = vec!["-C", repo, "branch", name];
        if let Some(start) = start_point {
            args.push(start);
        }
        let res = self.run(args, StdinMode::Null, None, None).await?;
        self.ensure_success(&res)
    }

    async fn delete_branch(&self, repo: &str, name: &str, force: bool) -> Result<(), AppError> {
        let mut args = vec!["-C", repo, "branch"];
        if force {
            args.push("-D");
        } else {
            args.push("-d");
        }
        args.push(name);
        let res = self.run(args, StdinMode::Null, None, None).await?;
        self.ensure_success(&res)
    }

    async fn rename_branch(
        &self,
        repo: &str,
        old_name: &str,
        new_name: &str,
    ) -> Result<(), AppError> {
        let args = ["-C", repo, "branch", "-m", old_name, new_name];
        let res = self.run(args, StdinMode::Null, None, None).await?;
        self.ensure_success(&res)
    }

    async fn checkout_branch(&self, repo: &str, name: &str) -> Result<(), AppError> {
        // Also covers tags (detached HEAD) — the UI labels it accordingly.
        let args = ["-C", repo, "checkout", name];
        let res = self.run(args, StdinMode::Null, None, None).await?;
        self.ensure_success(&res)
    }

    async fn set_branch_upstream(
        &self,
        repo: &str,
        branch: &str,
        upstream: Option<&str>,
    ) -> Result<(), AppError> {
        let args: Vec<String> = match upstream {
            Some(u) => vec![
                "-C".to_string(),
                repo.to_string(),
                "branch".to_string(),
                "--set-upstream-to".to_string(),
                u.to_string(),
                branch.to_string(),
            ],
            None => vec![
                "-C".to_string(),
                repo.to_string(),
                "branch".to_string(),
                "--unset-upstream".to_string(),
                branch.to_string(),
            ],
        };
        let res = self.run(args, StdinMode::Null, None, None).await?;
        self.ensure_success(&res)
    }

    async fn list_tags(&self, repo: &str) -> Result<Vec<engine::TagInfo>, AppError> {
        let args = ["-C", repo, "tag", "-l", "--format=%(refname)%00%(refname:short)%00%(objectname)%00%(taggername)%00%(creatordate:iso)%00%(subject)"];
        let res = self.run(args, StdinMode::Null, None, None).await?;
        let tags = parse::parse_tags(&res.stdout);
        Ok(tags)
    }

    async fn create_tag(
        &self,
        repo: &str,
        name: &str,
        message: Option<&str>,
        target: &str,
    ) -> Result<(), AppError> {
        let mut args = vec!["-C", repo, "tag", name];
        if let Some(msg) = message {
            args.push("-a");
            args.push("-m");
            args.push(msg);
        }
        args.push(target);
        let res = self.run(args, StdinMode::Null, None, None).await?;
        self.ensure_success(&res)
    }

    async fn delete_tag(&self, repo: &str, name: &str) -> Result<(), AppError> {
        let args = ["-C", repo, "tag", "-d", name];
        let res = self.run(args, StdinMode::Null, None, None).await?;
        self.ensure_success(&res)
    }

    async fn list_stash(&self, repo: &str) -> Result<Vec<engine::StashEntry>, AppError> {
        // log-family formats need `%x00` for a literal NUL (`%00` is not a
        // placeholder there and would stay literal).
        let args = [
            "-C",
            repo,
            "stash",
            "list",
            "--format=%H%x00%gd%x00%gs%x00%cr",
        ];
        let res = self.run(args, StdinMode::Null, None, None).await?;
        let entries = parse::parse_stash(&res.stdout);
        Ok(entries)
    }

    async fn stash_push(&self, repo: &str, message: Option<&str>) -> Result<usize, AppError> {
        let mut args = vec!["-C", repo, "stash", "push", "-u"];
        if let Some(msg) = message {
            args.push("-m");
            args.push(msg);
        }
        let res = self.run(args, StdinMode::Null, None, None).await?;
        self.ensure_success(&res)?;
        let idx = extract_stash_index(&res.stderr).unwrap_or(0);
        Ok(idx)
    }

    async fn stash_apply(&self, repo: &str, index: usize) -> Result<(), AppError> {
        let args = [
            "-C",
            repo,
            "stash",
            "apply",
            &format!("stash@{{{}}}", index),
        ];
        let res = self.run(args, StdinMode::Null, None, None).await?;
        self.ensure_success(&res)
    }

    async fn stash_pop(&self, repo: &str, index: usize) -> Result<(), AppError> {
        let args = ["-C", repo, "stash", "pop", &format!("stash@{{{}}}", index)];
        let res = self.run(args, StdinMode::Null, None, None).await?;
        self.ensure_success(&res)
    }

    async fn stash_drop(&self, repo: &str, index: usize) -> Result<(), AppError> {
        let args = ["-C", repo, "stash", "drop", &format!("stash@{{{}}}", index)];
        let res = self.run(args, StdinMode::Null, None, None).await?;
        self.ensure_success(&res)
    }

    async fn list_remotes(&self, repo: &str) -> Result<Vec<engine::RemoteInfo>, AppError> {
        let args = ["-C", repo, "remote", "-v"];
        let res = self.run(args, StdinMode::Null, None, None).await?;
        let remotes = parse::parse_remotes(&res.stdout);
        Ok(remotes)
    }

    async fn add_remote(&self, repo: &str, name: &str, url: &str) -> Result<(), AppError> {
        let args = ["-C", repo, "remote", "add", name, url];
        let res = self.run(args, StdinMode::Null, None, None).await?;
        self.ensure_success(&res)
    }

    async fn remove_remote(&self, repo: &str, name: &str) -> Result<(), AppError> {
        let args = ["-C", repo, "remote", "remove", name];
        let res = self.run(args, StdinMode::Null, None, None).await?;
        self.ensure_success(&res)
    }

    async fn set_remote_url(
        &self,
        repo: &str,
        name: &str,
        url: &str,
        push: bool,
    ) -> Result<(), AppError> {
        let args: Vec<&str> = if push {
            vec!["-C", repo, "remote", "set-url", "--push", name, url]
        } else {
            vec!["-C", repo, "remote", "set-url", name, url]
        };
        let res = self.run(args, StdinMode::Null, None, None).await?;
        self.ensure_success(&res)
    }

    async fn prune_remote(&self, repo: &str, name: &str) -> Result<(), AppError> {
        let args = ["-C", repo, "remote", "prune", name];
        let res = self.run(args, StdinMode::Null, None, None).await?;
        self.ensure_success(&res)
    }

    async fn fetch(&self, repo: &str, remote: Option<&str>) -> Result<(), AppError> {
        let mut args = vec!["-C", repo, "fetch"];
        if let Some(r) = remote {
            args.push(r);
        }
        let res = self.run(args, StdinMode::Null, None, None).await?;
        ensure_success_net(&res)
    }

    async fn reflog(
        &self,
        repo: &str,
        ref_name: Option<&str>,
    ) -> Result<Vec<engine::ReflogEntry>, AppError> {
        let mut args = vec![
            "-C",
            repo,
            "reflog",
            "--format=%H%x00%h%x00%gd%x00%gs%x00%cr",
        ];
        if let Some(r) = ref_name {
            args.push(r);
        }
        let res = self.run(args, StdinMode::Null, None, None).await?;
        let entries = parse::parse_reflog(&res.stdout);
        Ok(entries)
    }

    async fn reset(&self, repo: &str, mode: &str, target: &str) -> Result<(), AppError> {
        let args = ["-C", repo, "reset", &format!("--{mode}"), target];
        let res = self.run(args, StdinMode::Null, None, None).await?;
        self.ensure_success(&res)
    }

    async fn apply(
        &self,
        repo: &str,
        patch: &str,
        cached: bool,
        reverse: bool,
    ) -> Result<(), AppError> {
        let mut args = vec!["-C", repo, "apply"];
        if cached {
            args.push("--cached");
        }
        if reverse {
            args.push("--reverse");
        }
        let res = self
            .run(args, StdinMode::Feed, Some(patch.as_bytes()), None)
            .await?;
        self.ensure_success(&res)
    }

    async fn clean_list(&self, repo: &str) -> Result<Vec<String>, AppError> {
        // Locale-independent equivalent of `git clean -nd` targets: untracked
        // files/dirs (dirs collapsed with a trailing `/`), ignored files
        // excluded — matching clean's default (no `-x`).
        let args = [
            "-C",
            repo,
            "ls-files",
            "--others",
            "--directory",
            "--exclude-standard",
            "-z",
        ];
        let res = self.run(args, StdinMode::Null, None, None).await?;
        self.ensure_success(&res)?;
        Ok(parse::parse_nul_paths(&res.stdout))
    }

    async fn clean(&self, repo: &str, paths: &[String]) -> Result<(), AppError> {
        if paths.is_empty() {
            return Ok(());
        }
        let mut args = vec!["-C", repo, "clean", "-fd", "--"];
        args.extend(paths.iter().map(|s| s.as_str()));
        let res = self.run(args, StdinMode::Null, None, None).await?;
        self.ensure_success(&res)
    }

    async fn merge_base(&self, repo: &str, a: &str, b: &str) -> Result<Option<String>, AppError> {
        let args = ["-C", repo, "merge-base", a, b];
        let res = self.run(args, StdinMode::Null, None, None).await?;
        match res.exit_code {
            Some(0) => {
                let line = res.stdout.lines().next().unwrap_or("").trim();
                if line.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(line.to_string()))
                }
            }
            // git exits 1 with "no merge base" — a result, not an error.
            Some(1) => Ok(None),
            Some(code) => Err(AppError::git_command(
                format!("git merge-base exited with code {code}"),
                res.stderr,
                res.stdout,
            )),
            None => Err(AppError::internal("git process terminated by signal")),
        }
    }

    async fn range_count(
        &self,
        repo: &str,
        left: &str,
        right: &str,
    ) -> Result<(u32, u32), AppError> {
        let spec = format!("{left}...{right}");
        let args = ["-C", repo, "rev-list", "--left-right", "--count", &spec];
        let res = self.run(args, StdinMode::Null, None, None).await?;
        self.ensure_success(&res)?;
        Ok(parse::parse_range_count(&res.stdout))
    }

    async fn rev_list(
        &self,
        repo: &str,
        range: &str,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<engine::CommitInfo>, AppError> {
        let mut args: Vec<String> = vec![
            "-C".to_string(),
            repo.to_string(),
            "log".to_string(),
            "--topo-order".to_string(),
            "--no-color".to_string(),
            format!("--format={LOG_FORMAT}"),
        ];
        if offset > 0 {
            args.push(format!("--skip={offset}"));
        }
        if limit > 0 {
            args.push(format!("-n{limit}"));
        }
        args.push(range.to_string());
        let args_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let res = self.run(args_refs, StdinMode::Null, None, None).await?;
        self.ensure_success(&res)?;
        Ok(parse::parse_log(&res.stdout))
    }

    async fn update_ref(&self, repo: &str, name: &str, target: &str) -> Result<(), AppError> {
        let args = ["-C", repo, "update-ref", name, target];
        let res = self.run(args, StdinMode::Null, None, None).await?;
        self.ensure_success(&res)
    }

    async fn list_backup_refs(&self, repo: &str) -> Result<Vec<engine::BackupRef>, AppError> {
        let args = [
            "-C",
            repo,
            "for-each-ref",
            "--format=%(refname)%00%(refname:short)%00%(objectname)%00%(objectname:short)%00%(creatordate:iso)%00%(subject)",
            "--sort=creatordate",
            "refs/ibexgit/backups/",
        ];
        let res = self.run(args, StdinMode::Null, None, None).await?;
        self.ensure_success(&res)?;
        Ok(parse::parse_backup_refs(&res.stdout))
    }

    async fn delete_backup_refs(&self, repo: &str, names: &[String]) -> Result<(), AppError> {
        for name in names {
            if !name.starts_with("refs/ibexgit/backups/") {
                return Err(AppError::parse(format!(
                    "refuse to delete non-backup ref {name:?}"
                )));
            }
            let args = ["-C", repo, "update-ref", "-d", name.as_str()];
            let res = self.run(args, StdinMode::Null, None, None).await?;
            self.ensure_success(&res)?;
        }
        Ok(())
    }

    async fn push(
        &self,
        repo: &str,
        remote: &str,
        branch: &str,
        force_with_lease: bool,
        set_upstream: bool,
        tags: bool,
    ) -> Result<(), AppError> {
        if !branch.is_empty() {
            let mut args: Vec<String> =
                vec!["-C".to_string(), repo.to_string(), "push".to_string()];
            if force_with_lease {
                args.push("--force-with-lease".to_string());
            }
            if set_upstream {
                args.push("--set-upstream".to_string());
            }
            args.push(remote.to_string());
            args.push(branch.to_string());
            let args_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
            let res = self.run(args_refs, StdinMode::Null, None, None).await?;
            ensure_success_net(&res)?;
        }
        if tags {
            let args = ["-C", repo, "push", remote, "--tags"];
            let res = self.run(args, StdinMode::Null, None, None).await?;
            ensure_success_net(&res)?;
        }
        if branch.is_empty() && !tags {
            return Err(AppError::parse(
                "push: nothing to push (no branch and no tags requested)",
            ));
        }
        Ok(())
    }

    async fn pull(
        &self,
        repo: &str,
        remote: Option<&str>,
        branch: Option<&str>,
        mode: Option<&str>,
    ) -> Result<engine::PullResult, AppError> {
        let mut args: Vec<String> = vec!["-C".to_string(), repo.to_string(), "pull".to_string()];
        match mode {
            Some("rebase") => args.push("--rebase".to_string()),
            Some("ff_only") => args.push("--ff-only".to_string()),
            _ => {}
        }
        if let Some(r) = remote {
            args.push(r.to_string());
        }
        if let Some(b) = branch {
            args.push(b.to_string());
        }
        let args_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let res = self.run(args_refs, StdinMode::Null, None, None).await?;
        // 凭据取消必须以错误浮出（任务=已取消），不折叠进 success=false。
        if res.exit_code != Some(0)
            && res
                .stderr
                .contains(crate::core::credential::CREDENTIAL_CANCELLED_MARKER)
        {
            return Err(AppError::CredentialCancelled);
        }
        let success = res.exit_code == Some(0);
        let combined = format!("{}{}", res.stdout, res.stderr);
        Ok(engine::PullResult {
            success,
            message: res.stderr,
            fast_forward: combined.contains("Fast-forward") || combined.contains("fast-forward"),
        })
    }

    async fn rebase(
        &self,
        repo: &str,
        target: &str,
        options: &[&str],
    ) -> Result<engine::RebaseState, AppError> {
        let mut args = vec!["-C", repo, "rebase"];
        for opt in options {
            args.push(*opt);
        }
        args.push(target);
        let res = self.run(args, StdinMode::Null, None, None).await?;
        self.ensure_success(&res)?;
        Ok(engine::RebaseState {
            state: "finished".to_string(),
            current_step: 0,
            total_steps: 0,
            current_commit: None,
        })
    }

    async fn merge(
        &self,
        repo: &str,
        target: &str,
        ff_only: bool,
    ) -> Result<engine::MergeResult, AppError> {
        let mut args: Vec<String> = vec!["-C".to_string(), repo.to_string(), "merge".to_string()];
        if ff_only {
            args.push("--ff-only".to_string());
        }
        args.push("--no-edit".to_string());
        args.push(target.to_string());
        let args_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let res = self.run(args_refs, StdinMode::Null, None, None).await?;
        let success = res.exit_code == Some(0);
        Ok(engine::MergeResult {
            success,
            message: if res.stderr.is_empty() {
                res.stdout
            } else {
                res.stderr
            },
        })
    }

    async fn blame(&self, repo: &str, _path: &str) -> Result<Vec<engine::ReflogEntry>, AppError> {
        // Blame lands in P4 (own BlameLine type); placeholder until then.
        let _ = repo;
        Err(AppError::not_implemented("blame"))
    }

    async fn clone_repo(
        &self,
        opts: &CloneOptions,
        cancel: Option<&CancelToken>,
        on_line: Option<Arc<dyn Fn(String) + Send + Sync>>,
    ) -> Result<(), AppError> {
        // 目标目录存在且非空 → 提前拒绝（git 的报错体验差且会留下半成品）。
        let dest = Path::new(&opts.dest);
        if dest.exists() {
            let non_empty = std::fs::read_dir(dest)
                .map(|mut it| it.next().is_some())
                .unwrap_or(false);
            if non_empty {
                return Err(AppError::parse(format!(
                    "destination path '{}' already exists and is not empty",
                    opts.dest
                )));
            }
        }

        let mut args: Vec<String> = vec!["clone".to_string(), "--progress".to_string()];
        if let Some(depth) = opts.depth {
            args.push("--depth".to_string());
            args.push(depth.to_string());
        }
        if opts.single_branch {
            args.push("--single-branch".to_string());
        }
        if opts.recurse_submodules {
            args.push("--recurse-submodules".to_string());
        }
        args.push("--".to_string());
        args.push(opts.url.clone());
        args.push(opts.dest.clone());
        let args_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

        // 克隆可能很慢：超时给足（1h），可取消才是主要退出路径。
        // 进度回调可选（无 UI 的场景如取消测试/服务端用例）。
        let res = match on_line {
            Some(on_line) => {
                self.runner
                    .run_streaming(&self.git_path, &args_refs, Some(3600), cancel, on_line)
                    .await
            }
            None => {
                self.runner
                    .run_with_token(
                        &self.git_path,
                        &args_refs,
                        StdinMode::Null,
                        None,
                        Some(3600),
                        cancel,
                    )
                    .await
            }
        };
        let res = match res {
            Ok(r) => r,
            Err(AppError::OperationCancelled) => {
                // 清理半成品目录（仅空目录/仅 .git 的场景安全）。
                let _ = std::fs::remove_dir_all(dest);
                return Err(AppError::OperationCancelled);
            }
            Err(e) => return Err(e),
        };
        ensure_success_net(&res)?;
        Ok(())
    }

    async fn init_repo(&self, path: &str) -> Result<(), AppError> {
        let args = ["-c", "init.defaultBranch=main", "init", "-q", path];
        let res = self.run(args, StdinMode::Null, None, None).await?;
        self.ensure_success(&res)
    }
}

// =====================
// Parsers (P1 minimal)
// =====================

fn extract_stash_index(stderr: &str) -> Option<usize> {
    for line in stderr.lines() {
        if line.contains("stash@{") {
            let start = line.find("stash@{")? + 7;
            let rest = &line[start..];
            let end = rest.find('}')?;
            let num = rest[..end].parse().ok()?;
            return Some(num);
        }
    }
    None
}

/// Join a status-provided relative path onto the worktree root, rejecting
/// traversal outside the repository (defense in depth before fs deletes).
fn safe_join(root: &str, rel: &str) -> Result<PathBuf, AppError> {
    let rel_path = Path::new(rel);
    if rel_path.is_absolute() {
        return Err(AppError::internal(format!(
            "absolute path not allowed: {rel}"
        )));
    }
    let mut depth = 0usize;
    for comp in rel_path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| AppError::internal(format!("path escapes repo: {rel}")))?;
            }
            Component::Normal(_) => depth += 1,
            Component::Prefix(_) | Component::RootDir => {
                return Err(AppError::internal(format!(
                    "absolute path not allowed: {rel}"
                )));
            }
        }
    }
    if depth == 0 {
        return Err(AppError::internal(format!("path escapes repo: {rel}")));
    }
    Ok(PathBuf::from(root).join(rel_path))
}
