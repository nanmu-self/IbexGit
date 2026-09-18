use crate::core::engine::{
    self, conflict, parse, stats, CloneOptions, CommitTemplate, ConfigEntry, DiffModel,
    DiffOptions, DiffSource, FileContent, GitignoreFile, NumstatCommit,
};
use crate::core::error::AppError;
use crate::core::runner::{CancelToken, GitProcessRunner, ProcessResult, StdinMode};
use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

/// Default global gitignore: `$XDG_CONFIG_HOME/git/ignore`, else
/// `~/.config/git/ignore` (git applies the same XDG rule on all platforms,
/// with HOME = %USERPROFILE% on Windows).
fn default_gitignore_path() -> Option<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return Some(PathBuf::from(xdg).join("git").join("ignore"));
        }
    }
    Some(expand_home(
        &Path::new("~").join(".config").join("git").join("ignore"),
    ))
}

/// Expand a leading `~` (or a bare `~` path) to the user home directory.
fn expand_home(path: &Path) -> PathBuf {
    let home = || {
        std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from)
    };
    if path == Path::new("~") {
        return home().unwrap_or_else(|| path.to_path_buf());
    }
    if let Ok(rest) = path.strip_prefix("~") {
        if let Some(home) = home() {
            return home.join(rest);
        }
    }
    path.to_path_buf()
}

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

    // =====================
    // P8 internals: unmerged index + stage blobs + worktree reads
    // =====================

    /// All unmerged index records (`git ls-files -u -z`), first-seen order.
    async fn unmerged_records(&self, repo: &str) -> Result<Vec<engine::IndexEntry>, AppError> {
        let res = self
            .run(
                ["-C", repo, "ls-files", "-u", "-z"],
                StdinMode::Null,
                None,
                None,
            )
            .await?;
        self.ensure_success(&res)?;
        Ok(parse::parse_ls_files(&res.stdout))
    }

    /// Group unmerged records by path (insertion order preserved).
    fn group_unmerged(
        records: &[engine::IndexEntry],
    ) -> Vec<(String, Vec<(u32, engine::IndexEntry)>)> {
        let mut groups: Vec<(String, Vec<(u32, engine::IndexEntry)>)> = Vec::new();
        for rec in records {
            match groups.iter_mut().find(|(p, _)| *p == rec.path) {
                Some((_, stages)) => stages.push((rec.stage, rec.clone())),
                None => groups.push((rec.path.clone(), vec![(rec.stage, rec.clone())])),
            }
        }
        groups
    }

    /// Blob contents for many shas in one spawn (`cat-file --batch`):
    /// missing/non-blob objects and oversized blobs come back as `None`.
    async fn batch_blobs(
        &self,
        repo: &str,
        shas: &[&str],
    ) -> Result<Vec<Option<Vec<u8>>>, AppError> {
        if shas.is_empty() {
            return Ok(Vec::new());
        }
        let mut input = String::new();
        for s in shas {
            input.push_str(s);
            input.push('\n');
        }
        let res = self
            .runner
            .run_raw(
                &self.git_path,
                &["-C", repo, "cat-file", "--batch"],
                StdinMode::Feed,
                Some(input.as_bytes()),
                Some(60),
                None,
            )
            .await?;
        if res.exit_code != Some(0) {
            return Err(AppError::git_command(
                "git cat-file --batch",
                res.stderr,
                String::new(),
            ));
        }
        // Parse the batch stream: "<sha> blob <size>\n<bytes>\n" per object;
        // absent objects produce "<sha> missing\n".
        let mut out = Vec::with_capacity(shas.len());
        let mut pos = 0usize;
        let buf = &res.stdout;
        for _ in 0..shas.len() {
            let Some(nl) = buf[pos..].iter().position(|&b| b == b'\n') else {
                break;
            };
            let header = String::from_utf8_lossy(&buf[pos..pos + nl]).to_string();
            pos += nl + 1;
            let mut it = header.split_whitespace();
            let sha = it.next().unwrap_or("");
            let ty = it.next().unwrap_or("");
            let size: usize = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            if ty != "blob" || size == 0 {
                out.push((sha.to_string(), None));
                continue;
            }
            if pos + size > buf.len() {
                break; // truncated stream: stop, remaining = None
            }
            let data = buf[pos..pos + size].to_vec();
            pos += size + 1; // trailing LF after the payload
            out.push((
                sha.to_string(),
                (data.len() as u64 <= conflict::MAX_SIDE_BYTES).then_some(data),
            ));
        }
        // Realign with the requested order (batch echoes input order, but
        // be defensive about short streams).
        Ok(shas
            .iter()
            .map(|s| {
                out.iter()
                    .find(|(echo, _)| echo == *s)
                    .and_then(|(_, b)| b.clone())
            })
            .collect())
    }

    /// `true` when the plain path under a git `~<label>` conflict suffix is
    /// a directory in the worktree (file/directory conflicts).
    fn has_dir_at_base_path(root: &str, path: &str) -> bool {
        let Some(pos) = path.rfind('~') else {
            return false;
        };
        let base = &path[..pos];
        if base.is_empty() {
            return false;
        }
        safe_join(root, base).map(|p| p.is_dir()).unwrap_or(false)
    }

    /// Worktree state of one path + capped bytes (`None` unless Ok).
    fn worktree_read(root: &str, path: &str) -> (conflict::WorktreeState, Option<Vec<u8>>) {
        let Ok(abs) = safe_join(root, path) else {
            return (conflict::WorktreeState::Missing, None);
        };
        match std::fs::metadata(&abs) {
            Ok(m) if m.is_dir() => (conflict::WorktreeState::Directory, None),
            Ok(m) => {
                if m.len() > conflict::MAX_SIDE_BYTES {
                    return (conflict::WorktreeState::Oversized, None);
                }
                match std::fs::read(&abs) {
                    Ok(b) => (conflict::WorktreeState::Ok, Some(b)),
                    Err(_) => (conflict::WorktreeState::Missing, None),
                }
            }
            Err(_) => (conflict::WorktreeState::Missing, None),
        }
    }

    /// Classification for every unmerged path (shared by list + model).
    async fn all_conflict_models(
        &self,
        repo: &str,
    ) -> Result<Vec<conflict::ConflictModel>, AppError> {
        let records = self.unmerged_records(repo).await?;
        let groups = Self::group_unmerged(&records);
        if groups.is_empty() {
            return Ok(Vec::new());
        }
        // One batch read for every referenced stage blob.
        let shas: Vec<&str> = groups
            .iter()
            .flat_map(|(_, stages)| stages.iter().map(|(_, e)| e.sha.as_str()))
            .collect();
        let blobs = self.batch_blobs(repo, &shas).await?;
        let mut it = blobs.into_iter();
        let mut out = Vec::with_capacity(groups.len());
        for (path, stages) in &groups {
            let sides: Vec<(u32, Option<Vec<u8>>)> = stages
                .iter()
                .map(|(s, _)| (*s, it.next().flatten()))
                .collect();
            let (state, worktree_bytes) = Self::worktree_read(repo, path);
            let sibling_directory = Self::has_dir_at_base_path(repo, path);
            out.push(conflict::build_model(
                path,
                &records,
                stages,
                state,
                worktree_bytes.as_deref(),
                sibling_directory,
                &sides,
            ));
        }
        Ok(out)
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
        // Match git's interactive cleanup: comment lines (e.g. the
        // "# Conflicts:" block pre-filled from MERGE_MSG, P8) are stripped.
        args.push("--cleanup=strip");
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

    async fn file_history(
        &self,
        repo: &str,
        path: &str,
        limit: u32,
        start: Option<&str>,
    ) -> Result<Vec<engine::FileCommit>, AppError> {
        // --follow tracks the file across renames; --name-status -z carries
        // the per-commit path (rename entries: status/old/new) so the UI can
        // fetch the right diff for every entry. See parse_file_history for
        // the exact record shape.
        let mut args: Vec<String> = vec![
            "-C".into(),
            repo.into(),
            "log".into(),
            "--follow".into(),
            "--no-color".into(),
            "--format=%H%x00%h%x00%an%x00%ae%x00%aI%x00%s%x00%P".into(),
            "--name-status".into(),
            "-z".into(),
        ];
        if limit > 0 {
            // One extra record when paginating: the cursor commit itself,
            // dropped below (needed so the rename link at the boundary is
            // re-detected — `--skip` miscounts under --follow).
            args.push("-n".into());
            args.push((limit + u32::from(start.is_some())).to_string());
        }
        if let Some(s) = start {
            args.push(s.to_string());
        }
        args.push("--".into());
        args.push(path.into());
        let args_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let res = self.run(args_refs, StdinMode::Null, None, None).await?;
        if res.exit_code != Some(0) {
            // Unborn HEAD (fresh repository): empty history, not an error.
            if res.stderr.contains("does not have any commits") {
                return Ok(Vec::new());
            }
            return Err(AppError::git_command(
                format!("git exited with code {:?}", res.exit_code),
                res.stderr,
                res.stdout,
            ));
        }
        let mut commits = parse::parse_file_history(&res.stdout);
        if start.is_some() && !commits.is_empty() {
            // Defensive: only drop when the cursor really heads the page.
            if commits[0].hash == start.unwrap_or_default() {
                commits.remove(0);
            }
        }
        Ok(commits)
    }

    async fn blame(&self, repo: &str, path: &str) -> Result<engine::BlameResult, AppError> {
        // Porcelain output is locale-independent; paths arrive raw UTF-8
        // (core.quotepath=false is forced by the runner).
        let res = self
            .run(
                ["-C", repo, "blame", "--porcelain", "--", path],
                StdinMode::Null,
                None,
                None,
            )
            .await?;
        self.ensure_success(&res)?;
        Ok(parse::parse_blame(&res.stdout))
    }

    // =====================
    // P8: conflicts & operation state
    // =====================

    async fn conflict_list(&self, repo: &str) -> Result<Vec<conflict::ConflictSummary>, AppError> {
        let models = self.all_conflict_models(repo).await?;
        Ok(models
            .iter()
            .map(|m| conflict::ConflictSummary {
                path: m.path.clone(),
                code: m.code.clone(),
                conflict_type: m.conflict_type,
                block_count: m.block_count,
                binary: m.binary,
                oversized: m.oversized,
                directory: m.directory,
                submodule: m.submodule,
                editable: m.editable,
            })
            .collect())
    }

    async fn conflict_model(
        &self,
        repo: &str,
        path: &str,
    ) -> Result<conflict::ConflictModel, AppError> {
        let all = self.all_conflict_models(repo).await?;
        all.into_iter()
            .find(|m| m.path == path)
            .ok_or_else(|| AppError::parse(format!("path {path:?} has no unmerged index entry")))
    }

    async fn resolve_conflict_text(
        &self,
        repo: &str,
        path: &str,
        text: &str,
    ) -> Result<(), AppError> {
        conflict::validate_resolved(text)?;
        let abs = safe_join(repo, path)?;
        if abs.is_dir() {
            return Err(AppError::parse(format!(
                "cannot write {path:?}: the worktree path is a directory"
            )));
        }
        clear_readonly(&abs);
        std::fs::write(&abs, text.as_bytes())?;
        self.stage(repo, &[path.to_string()]).await
    }

    async fn resolve_conflict_keep(
        &self,
        repo: &str,
        path: &str,
        side: &str,
    ) -> Result<(), AppError> {
        let stage_no = match side {
            "ours" => 2u32,
            "theirs" => 3,
            other => return Err(AppError::parse(format!("unknown conflict side {other:?}"))),
        };
        let records = self.unmerged_records(repo).await?;
        let stages = Self::group_unmerged(&records)
            .into_iter()
            .find(|(p, _)| p == path)
            .map(|(_, s)| s)
            .ok_or_else(|| AppError::parse(format!("path {path:?} has no unmerged index entry")))?;
        let entry = stages.iter().find(|(s, _)| *s == stage_no).ok_or_else(|| {
            AppError::parse(format!(
                "path {path:?} has no {side} (stage {stage_no}) entry"
            ))
        })?;
        let bytes = self
            .batch_blobs(repo, &[entry.1.sha.as_str()])
            .await?
            .into_iter()
            .next()
            .flatten()
            .ok_or_else(|| AppError::parse(format!("{side} side of {path:?} is unavailable")))?;

        // DirectoryFile: the directory occupying the path must go before a
        // file can be written back (frontend confirms the deletion).
        let abs = safe_join(repo, path)?;
        if abs.is_dir() {
            std::fs::remove_dir_all(&abs)?;
        }
        clear_readonly(&abs);
        std::fs::write(&abs, bytes)?;
        self.stage(repo, &[path.to_string()]).await
    }

    async fn resolve_conflict_delete(&self, repo: &str, path: &str) -> Result<(), AppError> {
        let abs = safe_join(repo, path)?;
        // DirectoryFile: the worktree holds a directory; only the index
        // entry can go (the directory belongs to the other side). A path
        // absent from the worktree (rename origin, DD) is index-only too.
        let cached = abs.is_dir() || !abs.exists();
        let mut args: Vec<String> = vec!["-C".into(), repo.into(), "rm".into(), "-f".into()];
        if cached {
            args.push("--cached".into());
        }
        args.push("--".into());
        args.push(path.into());
        let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let res = self.run(refs, StdinMode::Null, None, None).await?;
        self.ensure_success(&res)
    }

    async fn operation_state(
        &self,
        repo: &str,
    ) -> Result<Option<engine::OperationState>, AppError> {
        let git_dir = crate::core::watcher::resolve_git_dir(Path::new(repo)).ok_or_else(|| {
            AppError::InvalidRepo {
                path: repo.to_string(),
            }
        })?;
        Ok(detect_operation_state(&git_dir))
    }

    async fn operation_abort(&self, repo: &str) -> Result<(), AppError> {
        let state = self
            .operation_state(repo)
            .await?
            .ok_or_else(|| AppError::parse("no operation in progress"))?;
        let verb = match state.kind {
            engine::OperationKind::Merge => "merge",
            engine::OperationKind::Rebase => "rebase",
            engine::OperationKind::CherryPick => "cherry-pick",
            engine::OperationKind::Revert => "revert",
            engine::OperationKind::Apply => "am",
            engine::OperationKind::Bisect => "bisect",
        };
        let action = if state.kind == engine::OperationKind::Bisect {
            "reset"
        } else {
            "--abort"
        };
        let res = self
            .run(["-C", repo, verb, action], StdinMode::Null, None, None)
            .await?;
        self.ensure_success(&res)
    }

    async fn operation_continue(&self, repo: &str) -> Result<(), AppError> {
        let state = self
            .operation_state(repo)
            .await?
            .ok_or_else(|| AppError::parse("no operation in progress"))?;
        let (verb, action) = match state.kind {
            engine::OperationKind::Merge => ("merge", "--continue"),
            engine::OperationKind::Rebase => ("rebase", "--continue"),
            engine::OperationKind::CherryPick => ("cherry-pick", "--continue"),
            engine::OperationKind::Revert => ("revert", "--continue"),
            engine::OperationKind::Apply => ("am", "--continue"),
            // Bisect stepping is a dedicated UI, not a conflict flow.
            engine::OperationKind::Bisect => {
                return Err(AppError::parse("continue is not available while bisecting"))
            }
        };
        let res = self
            .run(["-C", repo, verb, action], StdinMode::Null, None, None)
            .await?;
        self.ensure_success(&res)
    }

    async fn operation_skip(&self, repo: &str) -> Result<(), AppError> {
        let state = self
            .operation_state(repo)
            .await?
            .ok_or_else(|| AppError::parse("no operation in progress"))?;
        let (verb, action) = match state.kind {
            engine::OperationKind::Rebase => ("rebase", "--skip"),
            engine::OperationKind::CherryPick => ("cherry-pick", "--skip"),
            engine::OperationKind::Revert => ("revert", "--skip"),
            engine::OperationKind::Apply => ("am", "--skip"),
            engine::OperationKind::Merge | engine::OperationKind::Bisect => {
                return Err(AppError::parse("skip is not available for this operation"))
            }
        };
        let res = self
            .run(["-C", repo, verb, action], StdinMode::Null, None, None)
            .await?;
        self.ensure_success(&res)
    }

    async fn mergetool(
        &self,
        repo: &str,
        path: &str,
        tool: Option<&str>,
        cmd: Option<&str>,
    ) -> Result<(), AppError> {
        // GUI tools stay open for minutes; prompts are disabled and the
        // tool's exit code is trusted (both via -c, nothing persisted).
        const MERGETOOL_TIMEOUT_SECS: u64 = 60 * 60;
        let tool_name = match (tool, cmd) {
            (Some(t), _) => {
                if t.is_empty()
                    || !t
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                {
                    return Err(AppError::parse(format!("invalid merge tool name {t:?}")));
                }
                t.to_string()
            }
            (None, Some(_)) => "ibexgit-custom".to_string(),
            (None, None) => return Err(AppError::parse("mergetool: no tool configured")),
        };
        let mut args: Vec<String> = vec![
            "-C".into(),
            repo.into(),
            "-c".into(),
            format!("merge.tool={tool_name}"),
            "-c".into(),
            "mergetool.prompt=false".into(),
            "-c".into(),
            format!("mergetool.{tool_name}.trustExitCode=true"),
            "-c".into(),
            "mergetool.keepBackup=false".into(),
        ];
        if let Some(c) = cmd {
            args.push("-c".into());
            args.push(format!("mergetool.{tool_name}.cmd={c}"));
        }
        args.push("mergetool".into());
        args.push("--".into());
        args.push(path.into());
        let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let res = self
            .run(refs, StdinMode::Null, None, Some(MERGETOOL_TIMEOUT_SECS))
            .await?;
        self.ensure_success(&res)
    }

    // ---- P10: 配置查看器 / 提交模板（只读） ----

    async fn config_global(&self) -> Result<Vec<ConfigEntry>, AppError> {
        let res = self
            .run(
                ["config", "--list", "-z", "--global"],
                StdinMode::Null,
                None,
                None,
            )
            .await?;
        self.ensure_success(&res)?;
        Ok(parse::parse_config_list_z(&res.stdout)
            .into_iter()
            .map(|(key, value)| ConfigEntry { key, value })
            .collect())
    }

    async fn config_local(&self, repo: &str) -> Result<Vec<ConfigEntry>, AppError> {
        let res = self
            .run(
                ["-C", repo, "config", "--list", "-z", "--local"],
                StdinMode::Null,
                None,
                None,
            )
            .await?;
        self.ensure_success(&res)?;
        Ok(parse::parse_config_list_z(&res.stdout)
            .into_iter()
            .map(|(key, value)| ConfigEntry { key, value })
            .collect())
    }

    async fn config_set_global(&self, key: &str, value: Option<&str>) -> Result<(), AppError> {
        if !parse::config_key_valid(key) {
            return Err(AppError::parse(format!("invalid config key: {key:?}")));
        }
        // `--` 隔离 positional 参数：key 已校验不以 `-` 开头，value 也可能是
        // 任意字符串（如以 `-` 开头的代理地址）。
        let res = match value {
            Some(v) => {
                self.run(
                    ["config", "--global", "--", key, v],
                    StdinMode::Null,
                    None,
                    None,
                )
                .await?
            }
            None => {
                // exit 5 = key 本来就不存在 → 视为幂等成功。
                let res = self
                    .run(
                        ["config", "--global", "--unset", "--", key],
                        StdinMode::Null,
                        None,
                        None,
                    )
                    .await?;
                if res.exit_code == Some(5) {
                    return Ok(());
                }
                res
            }
        };
        self.ensure_success(&res)
    }

    async fn config_merged(&self, repo: &str) -> Result<Vec<ConfigEntry>, AppError> {
        let res = self
            .run(
                ["-C", repo, "config", "--list", "-z"],
                StdinMode::Null,
                None,
                None,
            )
            .await?;
        self.ensure_success(&res)?;
        Ok(parse::parse_config_list_z(&res.stdout)
            .into_iter()
            .map(|(key, value)| ConfigEntry { key, value })
            .collect())
    }

    async fn config_set_local(
        &self,
        repo: &str,
        key: &str,
        value: Option<&str>,
    ) -> Result<(), AppError> {
        if !parse::config_key_valid(key) {
            return Err(AppError::parse(format!("invalid config key: {key:?}")));
        }
        // `--` 隔离 positional 参数（key 已校验不以 `-` 开头，value 任意）；
        // `--replace-all`/`--unset-all`：同名键多行时全量替换/移除。
        let res = match value {
            Some(v) => {
                self.run(
                    [
                        "-C",
                        repo,
                        "config",
                        "--local",
                        "--replace-all",
                        "--",
                        key,
                        v,
                    ],
                    StdinMode::Null,
                    None,
                    None,
                )
                .await?
            }
            None => {
                // exit 5 = key 本来就不存在 → 视为幂等成功。
                let res = self
                    .run(
                        ["-C", repo, "config", "--local", "--unset-all", "--", key],
                        StdinMode::Null,
                        None,
                        None,
                    )
                    .await?;
                if res.exit_code == Some(5) {
                    return Ok(());
                }
                res
            }
        };
        self.ensure_success(&res)
    }

    async fn global_gitignore(&self) -> Result<Option<GitignoreFile>, AppError> {
        // `--type=path` expands `~`; unset → exit 1 with empty output.
        let res = self
            .run(
                [
                    "config",
                    "--global",
                    "--type=path",
                    "--get",
                    "core.excludesFile",
                ],
                StdinMode::Null,
                None,
                None,
            )
            .await?;
        let configured = if res.exit_code == Some(0) {
            let v = res.stdout.trim();
            (!v.is_empty()).then(|| PathBuf::from(v))
        } else {
            None
        };
        let path = configured.or_else(default_gitignore_path);
        let Some(path) = path else {
            return Ok(None);
        };
        let Ok(bytes) = std::fs::read(&path) else {
            return Ok(None); // default path absent — no global ignore, not an error
        };
        Ok(Some(GitignoreFile {
            path: path.display().to_string(),
            content: String::from_utf8_lossy(&bytes).into_owned(),
        }))
    }

    async fn commit_template(&self, repo: &str) -> Result<Option<CommitTemplate>, AppError> {
        let res = self
            .run(
                ["-C", repo, "config", "--get", "commit.template"],
                StdinMode::Null,
                None,
                None,
            )
            .await?;
        if res.exit_code != Some(0) {
            return Ok(None); // unset (exit 1) or config error → no template
        }
        let raw = res.stdout.trim();
        if raw.is_empty() {
            return Ok(None);
        }
        // git resolves commit.template relative to the worktree root; `~`
        // is NOT expanded for this (non path-typed) value — do both here.
        let mut path = expand_home(Path::new(raw));
        if path.is_relative() {
            path = Path::new(repo).join(path);
        }
        let bytes = std::fs::read(&path)
            .map_err(|e| AppError::io_with_detail("read commit template", e.to_string()))?;
        Ok(Some(CommitTemplate {
            path: path.display().to_string(),
            content: String::from_utf8_lossy(&bytes).into_owned(),
        }))
    }

    // ---- P11: AI 报告采集（Log 家族变体，只读） ----

    async fn log_numstat(
        &self,
        repo: &str,
        since: Option<&str>,
        until: Option<&str>,
        author: Option<&str>,
        limit: u32,
    ) -> Result<Vec<NumstatCommit>, AppError> {
        // --all：日报/周报覆盖所有分支上的工作；合并提交只有元信息、无 numstat
        //（git 对 merge 默认不输出 stat）。-z：路径 raw UTF-8、rename 布局可掌。
        let mut args = vec![
            "-C".to_string(),
            repo.to_string(),
            "log".to_string(),
            "--all".to_string(),
            "--no-color".to_string(),
            "-z".to_string(),
            "--numstat".to_string(),
            "--format=%x1e%H%x1f%an%x1f%ae%x1f%aI%x1f%s".to_string(),
        ];
        if let Some(s) = since {
            args.push("--since".to_string());
            args.push(s.to_string());
        }
        if let Some(u) = until {
            args.push("--until".to_string());
            args.push(u.to_string());
        }
        if let Some(a) = author {
            args.push("--author".to_string());
            args.push(a.to_string());
        }
        if limit > 0 {
            args.push("-n".to_string());
            args.push(limit.to_string());
        }
        let args_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let res = self.run(&args_refs, StdinMode::Null, None, None).await?;
        self.ensure_success(&res)?;
        Ok(parse::parse_log_numstat(&res.stdout))
    }

    async fn commit_stats(
        &self,
        repo: &str,
        rev: &str,
    ) -> Result<engine::CommitStatsDto, AppError> {
        // 只取作者与 committer 时间（%ct，unix 秒），聚合在 stats.rs：
        // 记录以 \x1e 开头、字段以 \x1f 分隔；--no-merges 对齐 GitHub 口径。
        let args = [
            "-C",
            repo,
            "log",
            "--no-color",
            "--no-merges",
            rev,
            "--format=%x1e%an%x1f%ae%x1f%ct",
        ];
        let res = self.run(args, StdinMode::Null, None, None).await?;
        if res.exit_code != Some(0) {
            // 无提交仓库（unborn HEAD）→ 空集而非错误；rev 真无效则照常报错。
            let head = self
                .run(
                    ["-C", repo, "rev-parse", "--verify", "--quiet", "HEAD"],
                    StdinMode::Null,
                    None,
                    None,
                )
                .await?;
            if head.exit_code != Some(0) {
                return Ok(stats::aggregate_stats(&[], chrono::Local::now()));
            }
            self.ensure_success(&res)?;
        }
        let points = stats::parse_stats_log(&res.stdout);
        Ok(stats::aggregate_stats(&points, chrono::Local::now()))
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

/// Clear a read-only worktree file so an engine write can proceed
/// (git-checked-out files are read-only when the index says so).
#[allow(clippy::permissions_set_readonly_false)] // Windows: the intended API
fn clear_readonly(path: &Path) {
    if let Ok(meta) = std::fs::metadata(path) {
        let perms = meta.permissions();
        if perms.readonly() {
            #[cfg(windows)]
            {
                let mut p = perms;
                p.set_readonly(false);
                let _ = std::fs::set_permissions(path, p);
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(path, perms.set_mode(0o644));
            }
        }
    }
}

fn read_trimmed(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn read_num(path: &Path) -> Option<u32> {
    read_trimmed(path).and_then(|s| s.parse().ok())
}

/// Detect the in-progress operation from `.git` state files (P8 仓库状态
/// 头). Pure fs inspection — unit-tested against synthetic git dirs.
///
/// Priority matters: a rebase conflict also writes CHERRY_PICK_HEAD, and
/// merge/cherry-pick/revert all write MERGE_MSG, so the sequence
/// directories are checked before the single-head markers.
pub(crate) fn detect_operation_state(git_dir: &Path) -> Option<engine::OperationState> {
    use engine::OperationKind;

    let rebase_merge = git_dir.join("rebase-merge");
    if rebase_merge.is_dir() {
        return Some(engine::OperationState {
            kind: OperationKind::Rebase,
            onto: read_trimmed(&rebase_merge.join("onto"))
                .or_else(|| read_trimmed(&git_dir.join("REBASE_HEAD"))),
            step: read_num(&rebase_merge.join("msgnum")),
            total: read_num(&rebase_merge.join("end")),
            message: None,
        });
    }

    let rebase_apply = git_dir.join("rebase-apply");
    if rebase_apply.is_dir() {
        // `applying` present → a `git am` in flight; otherwise rebase --apply.
        let kind = if rebase_apply.join("applying").exists() {
            OperationKind::Apply
        } else {
            OperationKind::Rebase
        };
        return Some(engine::OperationState {
            kind,
            onto: read_trimmed(&rebase_apply.join("onto"))
                .or_else(|| read_trimmed(&git_dir.join("REBASE_HEAD"))),
            step: read_num(&rebase_apply.join("msgnum"))
                .or_else(|| read_num(&rebase_apply.join("next"))),
            total: read_num(&rebase_apply.join("last"))
                .or_else(|| read_num(&rebase_apply.join("end"))),
            message: None,
        });
    }

    if git_dir.join("MERGE_HEAD").exists() {
        return Some(engine::OperationState {
            kind: OperationKind::Merge,
            onto: read_trimmed(&git_dir.join("MERGE_HEAD")),
            step: None,
            total: None,
            message: read_trimmed(&git_dir.join("MERGE_MSG")),
        });
    }
    if git_dir.join("CHERRY_PICK_HEAD").exists() {
        return Some(engine::OperationState {
            kind: OperationKind::CherryPick,
            onto: read_trimmed(&git_dir.join("CHERRY_PICK_HEAD")),
            step: None,
            total: None,
            message: read_trimmed(&git_dir.join("MERGE_MSG")),
        });
    }
    if git_dir.join("REVERT_HEAD").exists() {
        return Some(engine::OperationState {
            kind: OperationKind::Revert,
            onto: read_trimmed(&git_dir.join("REVERT_HEAD")),
            step: None,
            total: None,
            message: read_trimmed(&git_dir.join("MERGE_MSG")),
        });
    }
    if git_dir.join("BISECT_LOG").exists() {
        return Some(engine::OperationState {
            kind: OperationKind::Bisect,
            onto: None,
            step: None,
            total: None,
            message: None,
        });
    }
    None
}

#[cfg(test)]
mod op_state_tests {
    use super::*;

    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ibexgit-opstate-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn none_when_clean() {
        let dir = tempdir();
        assert!(detect_operation_state(&dir).is_none());
    }

    #[test]
    fn merge_state_with_message() {
        let dir = tempdir();
        std::fs::write(dir.join("MERGE_HEAD"), "abc123\n").unwrap();
        std::fs::write(
            dir.join("MERGE_MSG"),
            "Merge branch 'feat'\n\n# Conflicts:\n#\tf.txt\n",
        )
        .unwrap();
        let st = detect_operation_state(&dir).unwrap();
        assert_eq!(st.kind, engine::OperationKind::Merge);
        assert_eq!(st.onto.as_deref(), Some("abc123"));
        assert_eq!(
            st.message.as_deref(),
            Some("Merge branch 'feat'\n\n# Conflicts:\n#\tf.txt")
        );
    }

    #[test]
    fn rebase_merge_steps() {
        let dir = tempdir();
        let rm = dir.join("rebase-merge");
        std::fs::create_dir_all(&rm).unwrap();
        std::fs::write(rm.join("msgnum"), "2\n").unwrap();
        std::fs::write(rm.join("end"), "5\n").unwrap();
        std::fs::write(rm.join("onto"), "deadbeef\n").unwrap();
        // a rebase conflict also leaves CHERRY_PICK_HEAD behind — rebase wins
        std::fs::write(dir.join("CHERRY_PICK_HEAD"), "xyz\n").unwrap();
        let st = detect_operation_state(&dir).unwrap();
        assert_eq!(st.kind, engine::OperationKind::Rebase);
        assert_eq!(st.step, Some(2));
        assert_eq!(st.total, Some(5));
        assert_eq!(st.onto.as_deref(), Some("deadbeef"));
    }

    #[test]
    fn cherry_pick_and_revert() {
        let dir = tempdir();
        std::fs::write(dir.join("CHERRY_PICK_HEAD"), "cp1\n").unwrap();
        assert_eq!(
            detect_operation_state(&dir).unwrap().kind,
            engine::OperationKind::CherryPick
        );
        std::fs::remove_file(dir.join("CHERRY_PICK_HEAD")).unwrap();
        std::fs::write(dir.join("REVERT_HEAD"), "rv1\n").unwrap();
        assert_eq!(
            detect_operation_state(&dir).unwrap().kind,
            engine::OperationKind::Revert
        );
    }

    #[test]
    fn rebase_apply_is_rebase_but_am_is_apply() {
        let dir = tempdir();
        let ra = dir.join("rebase-apply");
        std::fs::create_dir_all(&ra).unwrap();
        std::fs::write(ra.join("next"), "1\n").unwrap();
        std::fs::write(ra.join("last"), "3\n").unwrap();
        assert_eq!(
            detect_operation_state(&dir).unwrap().kind,
            engine::OperationKind::Rebase
        );
        std::fs::write(ra.join("applying"), "").unwrap();
        assert_eq!(
            detect_operation_state(&dir).unwrap().kind,
            engine::OperationKind::Apply
        );
    }
}
