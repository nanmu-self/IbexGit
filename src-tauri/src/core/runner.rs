use crate::core::error::AppError;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::time::timeout;

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

/// GitProcessRunner: spawn / cancel / kill / wait / streams / stdin / timeout / env.
pub struct GitProcessRunner {
    /// Default timeout for operations (seconds).
    default_timeout_secs: u64,
    /// Force UTF-8 locale.
    utf8_env: bool,
    /// Disable terminal prompts globally.
    no_prompt: bool,
}

impl GitProcessRunner {
    pub fn new(default_timeout_secs: u64) -> Self {
        Self {
            default_timeout_secs,
            utf8_env: true,
            no_prompt: true,
        }
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
        let mut cmd = self.base_command(git);
        cmd.args(args);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        match stdin_mode {
            StdinMode::Null => {
                cmd.stdin(Stdio::null());
            }
            StdinMode::Feed => {
                if stdin_bytes.is_some() {
                    cmd.stdin(Stdio::piped());
                } else {
                    cmd.stdin(Stdio::null());
                }
            }
        }

        let mut child = cmd.spawn().map_err(|e| RunnerError::Spawn(e.to_string()))?;

        // If Feed mode, write stdin and close.
        if let (StdinMode::Feed, Some(bytes)) = (stdin_mode, stdin_bytes) {
            if let Some(mut stdin) = child.stdin.take() {
                stdin
                    .write_all(bytes)
                    .map_err(|e| RunnerError::Io(e.to_string()))?;
            }
        }

        let timeout_dur = Duration::from_secs(timeout_secs.unwrap_or(self.default_timeout_secs));
        let started_at = Instant::now();

        let child = Arc::new(tokio::sync::Mutex::new(Some(child)));
        let wait_future = {
            let child = child.clone();
            async move {
                let mut guard = child.lock().await;
                let child = guard.take().expect("child already taken");
                let output = child
                    .wait_with_output()
                    .map_err(|e| RunnerError::Io(e.to_string()))?;
                let duration_ms = started_at.elapsed().as_millis() as u64;
                let stdout = String::from_utf8(output.stdout).map_err(|_| RunnerError::Utf8)?;
                let stderr = String::from_utf8(output.stderr).map_err(|_| RunnerError::Utf8)?;
                Ok::<ProcessResult, RunnerError>(ProcessResult {
                    exit_code: output.status.code(),
                    stdout,
                    stderr,
                    duration_ms,
                })
            }
        };

        match timeout(timeout_dur, wait_future).await {
            Ok(res) => res.map_err(Into::<AppError>::into),
            Err(_) => {
                let mut guard = child.lock().await;
                if let Some(mut c) = guard.take() {
                    let _ = c.kill();
                }
                Err(RunnerError::Timeout(timeout_dur.as_secs()).into())
            }
        }
    }
}
