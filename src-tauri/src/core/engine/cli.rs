use crate::core::engine::{self, parse, DiffModel, DiffSource};
use crate::core::error::AppError;
use crate::core::runner::{GitProcessRunner, ProcessResult, StdinMode};

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
        Ok(parse::parse_status(&res.stdout))
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

    async fn discard(&self, repo: &str, paths: &[String]) -> Result<(), AppError> {
        if paths.is_empty() {
            return Ok(());
        }
        let mut args = vec!["-C", repo, "restore"];
        args.extend(paths.iter().map(|s| s.as_str()));
        let res = self.run(args, StdinMode::Null, None, None).await?;
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
    ) -> Result<DiffModel, AppError> {
        let mut args = vec![
            "-C",
            repo,
            "diff",
            "--no-color",
            "-M",
            "--diff-filter=ACDMRTUXB",
        ];
        match source {
            DiffSource::Staged => args.push("--cached"),
            DiffSource::Commit => {
                if let Some((a, b)) = rev_range {
                    args.push(a);
                    args.push(b);
                }
            }
            DiffSource::Stash => {
                args.push("stash");
            }
            DiffSource::Worktree => {}
        }
        if !paths.is_empty() {
            args.push("--");
            for p in paths {
                args.push(p.as_str());
            }
        }

        let res = self.run(args, StdinMode::Null, None, None).await?;
        let model = parse::parse_diff(&res.stdout, source, rev_range)?;
        Ok(model)
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
        let args = ["-C", repo, "checkout", name];
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
        let args = ["-C", repo, "stash", "list", "--format=%H%00%gd%00%gs%00%cr"];
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

    async fn fetch(&self, repo: &str, remote: Option<&str>) -> Result<(), AppError> {
        let mut args = vec!["-C", repo, "fetch"];
        if let Some(r) = remote {
            args.push(r);
        }
        let res = self.run(args, StdinMode::Null, None, None).await?;
        self.ensure_success(&res)
    }

    async fn reflog(
        &self,
        repo: &str,
        ref_name: Option<&str>,
    ) -> Result<Vec<engine::ReflogEntry>, AppError> {
        let mut args = vec!["-C", repo, "reflog", "--format=%H%00%h%00%gd%00%gs%00%cr"];
        if let Some(r) = ref_name {
            args.push(r);
        }
        let res = self.run(args, StdinMode::Null, None, None).await?;
        let entries = parse::parse_reflog(&res.stdout);
        Ok(entries)
    }

    async fn reset(&self, repo: &str, mode: &str, target: &str) -> Result<(), AppError> {
        let args = ["-C", repo, "reset", mode, target];
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

    async fn push(
        &self,
        repo: &str,
        remote: &str,
        branch: &str,
        force_with_lease: bool,
        set_upstream: bool,
    ) -> Result<(), AppError> {
        let mut args = vec!["-C", repo, "push"];
        if force_with_lease {
            args.push("--force-with-lease");
        }
        if set_upstream {
            args.push("--set-upstream");
        }
        args.push(remote);
        args.push(branch);
        let res = self.run(args, StdinMode::Null, None, None).await?;
        self.ensure_success(&res)
    }

    async fn pull(
        &self,
        repo: &str,
        remote: Option<&str>,
        branch: Option<&str>,
        strategy: Option<&str>,
    ) -> Result<engine::PullResult, AppError> {
        let mut args = vec!["-C", repo, "pull"];
        if let Some(r) = remote {
            args.push(r);
        }
        if let Some(b) = branch {
            args.push(b);
        }
        if let Some(s) = strategy {
            args.push("--strategy");
            args.push(s);
        }
        let res = self.run(args, StdinMode::Null, None, None).await?;
        let success = res.exit_code == Some(0);
        Ok(engine::PullResult {
            success,
            message: res.stderr,
            fast_forward: true,
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
        strategy: Option<&str>,
    ) -> Result<engine::MergeResult, AppError> {
        let mut args = vec!["-C", repo, "merge"];
        if let Some(s) = strategy {
            args.push("--strategy");
            args.push(s);
        }
        args.push(target);
        let res = self.run(args, StdinMode::Null, None, None).await?;
        let success = res.exit_code == Some(0);
        Ok(engine::MergeResult {
            success,
            message: res.stderr,
        })
    }

    async fn blame(&self, repo: &str, _path: &str) -> Result<Vec<engine::ReflogEntry>, AppError> {
        // Blame lands in P4 (own BlameLine type); placeholder until then.
        let _ = repo;
        Err(AppError::not_implemented("blame"))
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
