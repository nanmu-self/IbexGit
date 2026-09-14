use serde::{Deserialize, Serialize};
use std::fmt;
use ts_rs::TS;

/// Application-wide error type.
#[derive(Debug, Serialize, Deserialize, Clone, TS)]
#[ts(export, export_to = "../../src/lib/git/bindings/")]
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
            Self::Parse { message } => write!(f, "Parse error: {}", message),
            Self::Internal { message } => write!(f, "Internal error: {}", message),
            Self::NotImplemented { feature } => write!(f, "Not implemented: {}", feature),
        }
    }
}

impl std::error::Error for AppError {}

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
}
