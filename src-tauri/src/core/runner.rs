use crate::core::error::AppError;
use crate::core::proctree::TreeChild;
use std::io;
use std::process;
use std::sync::Arc;
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::watch;
use tokio::time::timeout;

/// 运行时可更新的 spawn 配置（P7）：凭据 helper 注入、代理、SSH。
/// 由 `app_set_net_config` 命令更新（前端设置），lib.rs 启动时注入凭据部分。
#[derive(Debug, Clone, Default)]
pub struct NetSpawnConfig {
    /// credential helper `-c` 参数对 + env 对（credential.rs::spawn_injection）。
    pub credential: Option<CredSpawnInjection>,
    /// inherit = 不设（默认）；none = 显式清除；custom = 设为用户值。
    pub proxy_mode: ProxyMode,
    pub proxy_url: Option<String>,
    /// 设置后 `GIT_SSH_COMMAND = "ssh -i <path> -o IdentitiesOnly=yes"`。
    pub ssh_key_path: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CredSpawnInjection {
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ProxyMode {
    #[default]
    Inherit,
    None,
    Custom,
}

impl ProxyMode {
    pub fn parse(s: &str) -> Self {
        match s {
            "none" => Self::None,
            "custom" => Self::Custom,
            _ => Self::Inherit,
        }
    }

    const PROXY_VARS: &'static [&'static str] = &[
        "http_proxy",
        "https_proxy",
        "all_proxy",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
    ];
}

/// 线程共享、运行时可变的 spawn 配置（CliEngine 构造时传入）。
#[derive(Clone, Default)]
pub struct SharedNetConfig(Arc<std::sync::RwLock<NetSpawnConfig>>);

impl SharedNetConfig {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn update(&self, f: impl FnOnce(&mut NetSpawnConfig)) {
        let mut cfg = self.0.write().unwrap();
        f(&mut cfg);
    }

    pub fn snapshot(&self) -> NetSpawnConfig {
        self.0.read().unwrap().clone()
    }
}

#[derive(Debug, Error)]
pub enum RunnerError {
    #[error("IO error: {0}")]
    Io(String),
    #[error("Process spawn failed: {0}")]
    Spawn(String),
    #[error("Process timed out after {0}s")]
    Timeout(u64),
    #[error("Process cancelled")]
    Cancelled,
    #[error("UTF-8 decode error")]
    Utf8,
}

impl From<RunnerError> for AppError {
    fn from(e: RunnerError) -> Self {
        match e {
            RunnerError::Io(s) => AppError::io(s),
            RunnerError::Spawn(s) => AppError::io_with_detail("spawn", s),
            RunnerError::Timeout(s) => AppError::internal(format!("timed out after {}s", s)),
            RunnerError::Cancelled => AppError::OperationCancelled,
            RunnerError::Utf8 => AppError::parse("UTF-8 decode error"),
        }
    }
}

/// Cooperative cancellation handle shared between the Task layer and a
/// running git process. `cancel()` is synchronous and may be called from
/// anywhere; clones observe the same state (watch channel, race-free).
///
/// 取消语义（PLAN §4.5）：运行中的进程经 Runner **终止进程确认后**才报
/// `Cancelled` —— 即先整树 kill，再 reap，然后才返回错误。
#[derive(Debug, Clone)]
pub struct CancelToken {
    inner: Arc<CancelInner>,
}

#[derive(Debug)]
struct CancelInner {
    tx: watch::Sender<bool>,
    rx: watch::Receiver<bool>,
}

impl CancelToken {
    pub fn new() -> Self {
        let (tx, rx) = watch::channel(false);
        Self {
            inner: Arc::new(CancelInner { tx, rx }),
        }
    }

    /// Signal cancellation. Idempotent.
    pub fn cancel(&self) {
        let _ = self.inner.tx.send(true);
    }

    pub fn is_cancelled(&self) -> bool {
        *self.inner.rx.borrow()
    }

    /// Resolves once (and only once) `cancel()` has been called.
    pub async fn cancelled(&self) {
        let _ = self.inner.rx.clone().wait_for(|cancelled| *cancelled).await;
    }
}

impl Default for CancelToken {
    fn default() -> Self {
        Self::new()
    }
}

/// Stdin mode for GitProcessRunner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StdinMode {
    /// stdin is connected to null device; any read from git will immediately error.
    Null,
    /// Write provided bytes to stdin once, then close.
    Feed,
}

/// Result of a completed git process.
#[derive(Debug, Clone)]
pub struct ProcessResult {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u64,
}

/// Binary-safe variant of [`ProcessResult`]: stdout is raw bytes (P4
/// `git cat-file blob` for image diff). stderr is lossily decoded for
/// diagnostics.
#[derive(Debug, Clone)]
pub struct RawResult {
    pub exit_code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: String,
    pub duration_ms: u64,
}

/// GitProcessRunner: spawn / cancel / kill / wait / streams / stdin / timeout / env.
///
/// 进程树 kill（PLAN P1）：cancel 与超时都经 [`proctree::TreeChild`] 终止
/// **整棵进程树** —— Windows 用 Job Object（`CREATE_SUSPENDED` → 收编 →
/// resume，`KILL_ON_JOB_CLOSE` 兜底），Unix 用进程组 `killpg(SIGKILL)`。
pub struct GitProcessRunner {
    /// Default timeout for operations (seconds).
    default_timeout_secs: u64,
    /// Force UTF-8 locale.
    utf8_env: bool,
    /// Disable terminal prompts globally.
    no_prompt: bool,
    /// P7 spawn 配置（凭据注入 / 代理 / SSH），运行时可变。
    net_config: SharedNetConfig,
}

enum RunOutcome {
    Done(Result<process::Output, RunnerError>),
    Cancelled,
}

impl GitProcessRunner {
    pub fn new(default_timeout_secs: u64) -> Self {
        Self {
            default_timeout_secs,
            utf8_env: true,
            no_prompt: true,
            net_config: SharedNetConfig::new(),
        }
    }

    /// 构造时指定共享 spawn 配置（P7：lib.rs 注入凭据 helper 配置）。
    pub fn with_net_config(default_timeout_secs: u64, net_config: SharedNetConfig) -> Self {
        Self {
            default_timeout_secs,
            utf8_env: true,
            no_prompt: true,
            net_config,
        }
    }

    pub fn net_config(&self) -> SharedNetConfig {
        self.net_config.clone()
    }

    /// Build a base command with common flags.
    fn base_command(&self, git: &str) -> Command {
        let mut cmd = Command::new(git);
        if self.utf8_env {
            cmd.env("LC_ALL", "C.UTF-8");
            cmd.env("LANG", "C.UTF-8");
        }
        if self.no_prompt {
            cmd.env("GIT_TERMINAL_PROMPT", "0");
            cmd.env("GIT_EDITOR", "true");
            cmd.env("GIT_PAGER", "cat");
        }

        // P7：凭据 helper / 代理 / SSH 注入（所有 git 进程统一生效）。
        let net = self.net_config.snapshot();
        if let Some(cred) = &net.credential {
            // 先空值重置再接自身 helper，屏蔽用户全局 helper 的过期凭据。
            for arg in &cred.args {
                cmd.arg(arg);
            }
            for (k, v) in &cred.env {
                cmd.env(k, v);
            }
        }
        match net.proxy_mode {
            ProxyMode::Inherit => {}
            ProxyMode::None => {
                for var in ProxyMode::PROXY_VARS {
                    cmd.env_remove(var);
                }
            }
            ProxyMode::Custom => {
                if let Some(url) = net.proxy_url.as_deref().filter(|s| !s.is_empty()) {
                    for var in ProxyMode::PROXY_VARS {
                        cmd.env(var, url);
                    }
                }
            }
        }
        if let Some(key) = net.ssh_key_path.as_deref().filter(|s| !s.is_empty()) {
            cmd.env(
                "GIT_SSH_COMMAND",
                format!("ssh -i {key} -o IdentitiesOnly=yes"),
            );
        }

        cmd.arg("-c").arg("core.quotepath=false");
        cmd
    }

    /// Run a git command to completion, returning stdout on success.
    pub async fn run(
        &self,
        git: &str,
        args: &[&str],
        stdin_mode: StdinMode,
        stdin_bytes: Option<&[u8]>,
        timeout_secs: Option<u64>,
    ) -> Result<ProcessResult, AppError> {
        self.run_with_token(git, args, stdin_mode, stdin_bytes, timeout_secs, None)
            .await
    }

    /// [`GitProcessRunner::run`] with a cancellation token. Cancellation
    /// terminates the whole process tree (Job Object / process group) and
    /// only then fails with `OperationCancelled`.
    pub async fn run_with_token(
        &self,
        git: &str,
        args: &[&str],
        stdin_mode: StdinMode,
        stdin_bytes: Option<&[u8]>,
        timeout_secs: Option<u64>,
        cancel: Option<&CancelToken>,
    ) -> Result<ProcessResult, AppError> {
        let out = self
            .run_raw(git, args, stdin_mode, stdin_bytes, timeout_secs, cancel)
            .await?;
        Ok(ProcessResult {
            exit_code: out.exit_code,
            stdout: String::from_utf8(out.stdout).map_err(|_| RunnerError::Utf8)?,
            stderr: out.stderr,
            duration_ms: out.duration_ms,
        })
    }

    /// Binary-safe run: stdout returned as raw bytes (P4 `cat-file blob`).
    pub async fn run_raw(
        &self,
        git: &str,
        args: &[&str],
        stdin_mode: StdinMode,
        stdin_bytes: Option<&[u8]>,
        timeout_secs: Option<u64>,
        cancel: Option<&CancelToken>,
    ) -> Result<RawResult, AppError> {
        let mut cmd = self.base_command(git);
        cmd.args(args);
        cmd.stdout(process::Stdio::piped());
        cmd.stderr(process::Stdio::piped());

        match stdin_mode {
            StdinMode::Null => {
                cmd.stdin(process::Stdio::null());
            }
            StdinMode::Feed => {
                if stdin_bytes.is_some() {
                    cmd.stdin(process::Stdio::piped());
                } else {
                    cmd.stdin(process::Stdio::null());
                }
            }
        }

        self.run_command_bytes(
            &mut cmd,
            stdin_mode,
            stdin_bytes,
            timeout_secs,
            cancel,
            None,
        )
        .await
    }

    /// 流式 run（P7）：stderr 逐行（含 `\r` 分段）转发给 `on_line`，
    /// 返回值仍含完整 stderr（clone/fetch 进度 + 错误信息两用）。
    pub async fn run_streaming(
        &self,
        git: &str,
        args: &[&str],
        timeout_secs: Option<u64>,
        cancel: Option<&CancelToken>,
        on_line: Arc<dyn Fn(String) + Send + Sync>,
    ) -> Result<ProcessResult, AppError> {
        let mut cmd = self.base_command(git);
        cmd.args(args);
        cmd.stdout(process::Stdio::piped());
        cmd.stderr(process::Stdio::piped());
        cmd.stdin(process::Stdio::null());

        let out = self
            .run_command_bytes(
                &mut cmd,
                StdinMode::Null,
                None,
                timeout_secs,
                cancel,
                Some(on_line),
            )
            .await?;
        Ok(ProcessResult {
            exit_code: out.exit_code,
            stdout: String::from_utf8(out.stdout).map_err(|_| RunnerError::Utf8)?,
            stderr: out.stderr,
            duration_ms: out.duration_ms,
        })
    }

    /// Execute an arbitrary command with the runner's timeout / cancel /
    /// tree-kill semantics. Public so the credential helper (P7) and tests
    /// can reuse the same底座 for non-git processes.
    pub async fn run_command(
        &self,
        cmd: &mut Command,
        stdin_mode: StdinMode,
        stdin_bytes: Option<&[u8]>,
        timeout_secs: Option<u64>,
        cancel: Option<&CancelToken>,
    ) -> Result<ProcessResult, AppError> {
        let out = self
            .run_command_bytes(cmd, stdin_mode, stdin_bytes, timeout_secs, cancel, None)
            .await?;
        Ok(ProcessResult {
            exit_code: out.exit_code,
            stdout: String::from_utf8(out.stdout).map_err(|_| RunnerError::Utf8)?,
            stderr: out.stderr,
            duration_ms: out.duration_ms,
        })
    }

    /// Core execution path returning raw stdout bytes. `stderr_forward`（P7）
    /// 提供时，stderr 逐行转发（clone 进度）且返回值仍含完整 stderr。
    async fn run_command_bytes(
        &self,
        cmd: &mut Command,
        stdin_mode: StdinMode,
        stdin_bytes: Option<&[u8]>,
        timeout_secs: Option<u64>,
        cancel: Option<&CancelToken>,
        stderr_forward: Option<Arc<dyn Fn(String) + Send + Sync>>,
    ) -> Result<RawResult, AppError> {
        // Safety net only: normal paths reap or kill explicitly. `kill_on_drop`
        // plus the Windows job's KILL_ON_JOB_CLOSE make even panic paths clean up.
        cmd.kill_on_drop(true);

        let started_at = Instant::now();
        let mut child = TreeChild::spawn(cmd)
            .await
            .map_err(|e| RunnerError::Spawn(e.to_string()))?;

        // Feed mode: write once, then close. Feeding runs concurrently so a
        // large payload cannot deadlock on a full pipe; if the overall
        // timeout fires, the killed process closes the pipe and the feed
        // task unwinds.
        if let (StdinMode::Feed, Some(bytes)) = (stdin_mode, stdin_bytes) {
            if let Some(mut stdin) = child.take_stdin() {
                let bytes = bytes.to_vec();
                tokio::spawn(async move {
                    let _ = stdin.write_all(&bytes).await;
                });
            }
        }

        // P7 流式 stderr：逐行转发 + 累积（替代 wait_with_output 的 stderr 收集）。
        let stderr_task = stderr_forward.map(|fwd| {
            let pipe = child.take_stderr();
            tokio::spawn(async move {
                let mut collected: Vec<u8> = Vec::new();
                if let Some(pipe) = pipe {
                    let mut reader = tokio::io::BufReader::new(pipe);
                    let mut buf = Vec::new();
                    loop {
                        buf.clear();
                        match reader.read_until(b'\n', &mut buf).await {
                            Ok(0) | Err(_) => break,
                            Ok(_) => {}
                        }
                        collected.extend_from_slice(&buf);
                        let chunk = String::from_utf8_lossy(&buf);
                        for seg in chunk.split('\r') {
                            let seg = seg.trim_end_matches('\n');
                            if !seg.is_empty() {
                                fwd(seg.to_string());
                            }
                        }
                    }
                }
                collected
            })
        });

        let timeout_dur = Duration::from_secs(timeout_secs.unwrap_or(self.default_timeout_secs));
        let cancel = cancel.cloned().unwrap_or_default();

        let outcome = timeout(timeout_dur, async {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => RunOutcome::Cancelled,
                output = child.wait_with_output() =>
                    RunOutcome::Done(decode(output)),
            }
        })
        .await;

        // The select future has been consumed above, so `child` is ours again.
        match outcome {
            // Timed out: kill the tree, reap, report.
            Err(_elapsed) => {
                child.kill_tree();
                let _ = child.wait().await;
                if let Some(t) = stderr_task {
                    let _ = t.await;
                }
                Err(RunnerError::Timeout(timeout_dur.as_secs()).into())
            }
            Ok(RunOutcome::Cancelled) => {
                child.kill_tree();
                let _ = child.wait().await;
                if let Some(t) = stderr_task {
                    let _ = t.await;
                }
                Err(RunnerError::Cancelled.into())
            }
            Ok(RunOutcome::Done(Err(e))) => {
                if let Some(t) = stderr_task {
                    let _ = t.await;
                }
                Err(e.into())
            }
            Ok(RunOutcome::Done(Ok(output))) => {
                let exit_code = output.status.code();
                let stderr = match stderr_task {
                    Some(t) => match t.await {
                        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                        Err(_) => String::from_utf8_lossy(&output.stderr).into_owned(),
                    },
                    None => String::from_utf8_lossy(&output.stderr).into_owned(),
                };
                Ok(RawResult {
                    exit_code,
                    stdout: output.stdout,
                    stderr,
                    duration_ms: started_at.elapsed().as_millis() as u64,
                })
            }
        }
    }
}

fn decode(output: io::Result<process::Output>) -> Result<process::Output, RunnerError> {
    output.map_err(|e| RunnerError::Io(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runner() -> GitProcessRunner {
        GitProcessRunner::new(60)
    }

    fn long_lived_cmd() -> Command {
        #[cfg(windows)]
        {
            // Single arg after /c: cmd.exe runs the whole string; ping output
            // is consumed by our pipe reader, so no redirection is needed.
            let mut cmd = Command::new("cmd");
            cmd.arg("/c").arg("ping -n 30 127.0.0.1");
            cmd
        }
        #[cfg(not(windows))]
        {
            let mut cmd = Command::new("sleep");
            cmd.arg("30");
            cmd
        }
    }

    /// `run_command` leaves stdio to the caller (generic底座); git-level
    /// tests need piped output for ProcessResult to be meaningful.
    fn piped(cmd: &mut Command) {
        cmd.stdout(process::Stdio::piped())
            .stderr(process::Stdio::piped());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn run_command_succeeds() {
        let r = runner();
        let mut cmd = Command::new("git");
        cmd.args(["--version"]);
        piped(&mut cmd);
        let res = r
            .run_command(&mut cmd, StdinMode::Null, None, Some(30), None)
            .await
            .expect("git --version must succeed");
        assert_eq!(res.exit_code, Some(0));
        assert!(res.stdout.contains("git version"), "{}", res.stdout);
        assert!(res.duration_ms > 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn timeout_kills_process_and_reports() {
        let r = runner();
        let mut cmd = long_lived_cmd();
        let started = Instant::now();
        let err = r
            .run_command(&mut cmd, StdinMode::Null, None, Some(1), None)
            .await
            .expect_err("must time out");
        // Killed well before the 30s command finishes.
        assert!(started.elapsed() < Duration::from_secs(10));
        assert!(matches!(
            err,
            AppError::Internal { .. } | AppError::Io { .. }
        ));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cancel_token_terminates_process_tree() {
        let r = runner();
        let token = CancelToken::new();
        let mut cmd = long_lived_cmd();

        let canceller = {
            let token = token.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(200)).await;
                token.cancel();
            })
        };

        let started = Instant::now();
        let err = r
            .run_command(&mut cmd, StdinMode::Null, None, Some(30), Some(&token))
            .await
            .expect_err("must be cancelled");
        canceller.await.unwrap();

        assert!(
            started.elapsed() < Duration::from_secs(10),
            "must kill promptly"
        );
        assert!(
            matches!(err, AppError::OperationCancelled),
            "expected OperationCancelled, got {err:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cancel_that_arrives_after_completion_loses_the_race() {
        // Process finishes before cancellation: result wins, cancel is moot.
        let r = runner();
        let token = CancelToken::new();
        let mut cmd = Command::new("git");
        cmd.args(["--version"]);
        piped(&mut cmd);
        let res = r
            .run_command(&mut cmd, StdinMode::Null, None, Some(30), Some(&token))
            .await
            .expect("completing process must succeed");
        assert_eq!(res.exit_code, Some(0));
        assert!(!token.is_cancelled());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn feed_mode_writes_stdin_and_closes() {
        let r = runner();
        // hash-object works outside a repository without -w.
        let res = r
            .run(
                "git",
                &["hash-object", "--stdin"],
                StdinMode::Feed,
                Some(b"ibexgit feed stdin\n"),
                Some(30),
            )
            .await
            .expect("hash-object must succeed");
        assert_eq!(res.exit_code, Some(0));
        let sha = res.stdout.trim();
        assert_eq!(sha.len(), 40, "expected 40-char object id, got {sha:?}");
    }

    #[test]
    fn cancel_token_state_transitions() {
        let token = CancelToken::new();
        assert!(!token.is_cancelled());
        token.cancel();
        assert!(token.is_cancelled());
        // Idempotent.
        token.cancel();
        assert!(token.is_cancelled());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cancelled_future_resolves_for_all_clones() {
        let token = CancelToken::new();
        let a = token.clone();
        let b = token.clone();
        let waiter = {
            let b = b.clone();
            tokio::spawn(async move { b.cancelled().await })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!waiter.is_finished(), "must not resolve before cancel");
        a.cancel();
        tokio::time::timeout(Duration::from_secs(2), waiter)
            .await
            .expect("clone must observe cancellation")
            .unwrap();
        assert!(b.is_cancelled());
    }
}
