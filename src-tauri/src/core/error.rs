use serde::{Deserialize, Serialize};
use std::fmt;

/// Application-wide error type.
#[derive(Debug, Serialize, Deserialize, Clone, specta::Type)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum AppError {
    #[serde(rename_all = "snake_case")]
    Io {
        source: String,
        detail: Option<String>,
    },

    #[serde(rename_all = "snake_case")]
    GitCommand {
        command: String,
        stderr: String,
        stdout: String,
        detail: Option<String>,
    },

    #[serde(rename_all = "snake_case")]
    GitVersionTooOld {
        found: String,
        required: String,
    },

    #[serde(rename_all = "snake_case")]
    InvalidRepo {
        path: String,
    },

    OperationCancelled,

    CredentialCancelled,

    /// The cached DiffModel referenced by a line-level operation is gone
    /// (invalidated by a watcher event or evicted). The UI must re-fetch
    /// the diff and retry (PLAN P4: 同源保证 — 不重建，宁可拒绝).
    DiffModelExpired,

    #[serde(rename_all = "snake_case")]
    Parse {
        message: String,
    },

    #[serde(rename_all = "snake_case")]
    Internal {
        message: String,
    },

    #[serde(rename_all = "snake_case")]
    NotImplemented {
        feature: String,
    },

    // ---- P11 AI（ADR-013）：失败降级与明确报错，不阻塞提交流程 ----
    /// AI 功能未启用（默认 opt-in 关闭，设置中开启）。
    AiDisabled,

    /// 配置不完整（未配模型 / Base URL / 密钥）或无效。
    #[serde(rename_all = "snake_case")]
    AiConfig {
        message: String,
    },

    /// 网络层失败：DNS / 连接 / 空闲超时（断网、Ollama 未启动、代理故障）。
    #[serde(rename_all = "snake_case")]
    AiNetwork {
        message: String,
    },

    /// 认证失败（HTTP 401/403）：key 缺失或无效。
    #[serde(rename_all = "snake_case")]
    AiAuth {
        status: u32,
        message: String,
    },

    /// 额度 / 频率限制（HTTP 429）。
    #[serde(rename_all = "snake_case")]
    AiRateLimit {
        status: u32,
        message: String,
    },

    /// 其他 Provider HTTP 错误（5xx、4xx 未细分）。
    #[serde(rename_all = "snake_case")]
    AiProvider {
        status: u32,
        message: String,
    },
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { source, .. } => write!(f, "IO error: {}", source),
            Self::GitCommand { command, .. } => write!(f, "Git command failed: {}", command),
            Self::GitVersionTooOld { found, required } => {
                write!(f, "Git version too old: {}, required: {}", found, required)
            }
            Self::InvalidRepo { path } => write!(f, "Invalid repository: {}", path),
            Self::OperationCancelled => write!(f, "Operation cancelled by user"),
            Self::CredentialCancelled => write!(f, "Credential cancelled by user"),
            Self::DiffModelExpired => write!(f, "Diff model expired; please refresh"),
            Self::Parse { message } => write!(f, "Parse error: {}", message),
            Self::Internal { message } => write!(f, "Internal error: {}", message),
            Self::NotImplemented { feature } => write!(f, "Not implemented: {}", feature),
            Self::AiDisabled => write!(f, "AI assistant is disabled (enable it in Settings → AI)"),
            Self::AiConfig { message } => write!(f, "AI configuration error: {}", message),
            Self::AiNetwork { message } => write!(f, "AI network error: {}", message),
            Self::AiAuth { status, message } => {
                write!(f, "AI authentication failed ({}): {}", status, message)
            }
            Self::AiRateLimit { status, message } => {
                write!(f, "AI rate limited ({}): {}", status, message)
            }
            Self::AiProvider { status, message } => {
                write!(f, "AI provider error ({}): {}", status, message)
            }
        }
    }
}

impl std::error::Error for AppError {}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        AppError::io(e.to_string())
    }
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        AppError::internal(format!("json: {e}"))
    }
}

impl AppError {
    pub fn io(source: impl Into<String>) -> Self {
        Self::Io {
            source: source.into(),
            detail: None,
        }
    }

    pub fn io_with_detail(source: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::Io {
            source: source.into(),
            detail: Some(detail.into()),
        }
    }

    pub fn git_command(
        command: impl Into<String>,
        stderr: impl Into<String>,
        stdout: impl Into<String>,
    ) -> Self {
        Self::GitCommand {
            command: command.into(),
            stderr: stderr.into(),
            stdout: stdout.into(),
            detail: None,
        }
    }

    pub fn git_command_with_detail(
        command: impl Into<String>,
        stderr: impl Into<String>,
        stdout: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self::GitCommand {
            command: command.into(),
            stderr: stderr.into(),
            stdout: stdout.into(),
            detail: Some(detail.into()),
        }
    }

    pub fn parse(message: impl Into<String>) -> Self {
        Self::Parse {
            message: message.into(),
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::Internal {
            message: message.into(),
        }
    }

    pub fn not_implemented(feature: impl Into<String>) -> Self {
        Self::NotImplemented {
            feature: feature.into(),
        }
    }

    pub fn ai_config(message: impl Into<String>) -> Self {
        Self::AiConfig {
            message: message.into(),
        }
    }

    pub fn ai_network(message: impl Into<String>) -> Self {
        Self::AiNetwork {
            message: message.into(),
        }
    }
}
