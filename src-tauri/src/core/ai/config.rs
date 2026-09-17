//! AI 配置与密钥存储（P11，ADR-013）。
//!
//! - **非敏感配置**存 `{appData}/ai/config.json`（`#[serde(default)]` 向前兼容）；
//! - **API Key 只存 OS keychain**（keyring crate，service = `IbexGit`，
//!   entry = `ai:api-key`，复用 P7 凭据基建模式），**绝不返回给 WebView**——
//!   前端任何接口都拿不到 key 明文，只有 `has_key` 布尔位。

use crate::core::error::AppError;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;

// =====================
// 类型
// =====================

/// Provider 类型矩阵（ADR-013 决策 4）：一个 OpenAI 兼容客户端覆盖
/// OpenAI / Ollama / DeepSeek / Qwen / Moonshot / vLLM / LM Studio；
/// Anthropic Messages API；自定义 HTTP（OpenAI 兼容 wire format + 完整 URL）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// 本地离线选项（设置页置顶，隐私红线：数据不出本机）。
    Ollama,
    /// OpenAI 及一切兼容端点。
    OpenAiCompatible,
    /// Anthropic Messages API。
    Anthropic,
    /// 自定义 HTTP：`base_url` 填**完整**端点 URL，请求体为 OpenAI 兼容格式。
    Custom,
}

/// 非敏感配置（持久化 + 前端可见）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(default)]
pub struct AiConfig {
    /// 隐私红线（ADR-013 决策 5）：默认 opt-in **关闭**。
    pub enabled: bool,
    pub provider: ProviderKind,
    /// Ollama/OpenAI 兼容：API 根（如 `http://localhost:11434/v1`）；
    /// Anthropic：API 根；Custom：**完整**端点 URL。
    pub base_url: String,
    /// 模型名（如 `qwen3:8b`、`gpt-4o-mini`、`claude-sonnet-4-5`）。
    pub model: String,
    /// 请求超时（秒）：连接超时 = min(10s, 此值)；流式空闲超时 = 此值。
    pub timeout_secs: u32,
    /// 提交消息是否用 Conventional Commits 风格（`type(scope): summary`）。
    pub conventional: bool,
    /// Custom provider 的鉴权头风格：`bearer` | `x_api_key` | `none`。
    pub custom_auth: String,
    /// 排除路径规则（glob，如 `*.env`）；对 diff 文件与 numstat 路径生效。
    pub exclude_patterns: Vec<String>,
    /// 仓库级排除（规范化路径）；报告采集跳过、提交消息生成直接拒绝。
    pub excluded_repos: Vec<String>,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: ProviderKind::Ollama,
            base_url: "http://localhost:11434/v1".into(),
            model: String::new(),
            timeout_secs: 60,
            conventional: true,
            custom_auth: "bearer".into(),
            exclude_patterns: default_exclude_patterns(),
            excluded_repos: Vec::new(),
        }
    }
}

/// 内置隐私排除规则（安全默认：宁多勿漏；用户可在设置中增删）。
pub fn default_exclude_patterns() -> Vec<String> {
    vec![
        "*.env",
        "*.env.*",
        "*.pem",
        "*.key",
        "*.pfx",
        "*.p12",
        "*.jks",
        "*.keystore",
        "id_rsa*",
        "id_ed25519*",
        "id_ecdsa*",
        ".npmrc",
        ".netrc",
        "*credentials*",
        "*secret*",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

// =====================
// 密钥存储
// =====================

/// 机密存储抽象（keyring 生产实现 + 测试内存实现）。
pub trait SecretStore: Send + Sync {
    fn get(&self) -> Result<Option<String>, String>;
    fn set(&self, value: &str) -> Result<(), String>;
    fn delete(&self) -> Result<(), String>;
}

const KEYRING_SERVICE: &str = "IbexGit";
const KEYRING_ENTRY: &str = "ai:api-key";

/// OS keychain（Windows 凭据管理器 / macOS Keychain / Linux Secret Service）。
pub struct KeyringSecretStore;

impl SecretStore for KeyringSecretStore {
    fn get(&self) -> Result<Option<String>, String> {
        let entry =
            keyring::Entry::new(KEYRING_SERVICE, KEYRING_ENTRY).map_err(|e| e.to_string())?;
        match entry.get_password() {
            Ok(v) => Ok(Some(v)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    fn set(&self, value: &str) -> Result<(), String> {
        let entry =
            keyring::Entry::new(KEYRING_SERVICE, KEYRING_ENTRY).map_err(|e| e.to_string())?;
        entry.set_password(value).map_err(|e| e.to_string())
    }

    fn delete(&self) -> Result<(), String> {
        let entry =
            keyring::Entry::new(KEYRING_SERVICE, KEYRING_ENTRY).map_err(|e| e.to_string())?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// 进程内存储（单测 / 无 keychain 环境）。
#[derive(Default)]
pub struct MemorySecretStore(Mutex<Option<String>>);

impl SecretStore for MemorySecretStore {
    fn get(&self) -> Result<Option<String>, String> {
        Ok(self.0.lock().unwrap().clone())
    }
    fn set(&self, value: &str) -> Result<(), String> {
        *self.0.lock().unwrap() = Some(value.to_string());
        Ok(())
    }
    fn delete(&self) -> Result<(), String> {
        *self.0.lock().unwrap() = None;
        Ok(())
    }
}

// =====================
// 应用状态
// =====================

/// Tauri 管理态：配置目录 + 共享网络配置（代理继承 P7）+ 机密存储。
pub struct AiState {
    data_dir: PathBuf,
    pub net: crate::core::runner::SharedNetConfig,
    secrets: Box<dyn SecretStore>,
}

impl AiState {
    pub fn new(data_dir: PathBuf, net: crate::core::runner::SharedNetConfig) -> Self {
        Self {
            data_dir,
            net,
            secrets: Box::new(KeyringSecretStore),
        }
    }

    /// 测试专用：注入机密存储实现（CI 无 keychain）。
    pub fn with_secrets(
        data_dir: PathBuf,
        net: crate::core::runner::SharedNetConfig,
        secrets: Box<dyn SecretStore>,
    ) -> Self {
        Self {
            data_dir,
            net,
            secrets,
        }
    }

    fn config_path(&self) -> PathBuf {
        self.data_dir.join("ai").join("config.json")
    }

    /// 读取配置；文件缺失/损坏 → 默认值（配置损坏不应打挂整个功能）。
    pub fn load_config(&self) -> AiConfig {
        let Ok(raw) = std::fs::read_to_string(self.config_path()) else {
            return AiConfig::default();
        };
        serde_json::from_str(&raw).unwrap_or_default()
    }

    pub fn save_config(&self, cfg: &AiConfig) -> Result<(), AppError> {
        let path = self.config_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| AppError::io_with_detail(e.to_string(), "create ai config dir"))?;
        }
        let json = serde_json::to_string_pretty(cfg)?;
        std::fs::write(&path, json)
            .map_err(|e| AppError::io_with_detail(e.to_string(), "write ai config"))?;
        Ok(())
    }

    /// Key 明文只在 Rust 侧流转；对外只暴露存在性。
    pub fn has_key(&self) -> bool {
        self.secrets
            .get()
            .map(|v| v.is_some_and(|s| !s.is_empty()))
            .unwrap_or(false)
    }

    pub fn key(&self) -> Result<Option<String>, AppError> {
        self.secrets
            .get()
            .map_err(|e| AppError::io_with_detail(e, "read ai key from keychain"))
    }

    pub fn set_key(&self, value: &str) -> Result<(), AppError> {
        self.secrets
            .set(value)
            .map_err(|e| AppError::io_with_detail(e, "write ai key to keychain"))
    }

    pub fn delete_key(&self) -> Result<(), AppError> {
        self.secrets
            .delete()
            .map_err(|e| AppError::io_with_detail(e, "delete ai key from keychain"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> AiState {
        let dir = std::env::temp_dir().join(format!("ibexgit-ai-test-{}", std::process::id()));
        AiState::with_secrets(
            dir,
            crate::core::runner::SharedNetConfig::new(),
            Box::new(MemorySecretStore::default()),
        )
    }

    #[test]
    fn default_config_is_opt_in_off() {
        let cfg = AiConfig::default();
        assert!(!cfg.enabled, "privacy: default must be disabled");
        assert_eq!(cfg.provider, ProviderKind::Ollama);
        assert!(!cfg.exclude_patterns.is_empty());
    }

    #[test]
    fn config_roundtrip_and_corruption_fallback() {
        let s = state();
        let cfg = AiConfig {
            model: "qwen3:8b".into(),
            enabled: true,
            ..AiConfig::default()
        };
        s.save_config(&cfg).unwrap();
        assert_eq!(s.load_config(), cfg);
        // 损坏文件 → 默认值。
        std::fs::create_dir_all(s.config_path().parent().unwrap()).unwrap();
        std::fs::write(s.config_path(), "{oops").unwrap();
        assert_eq!(s.load_config(), AiConfig::default());
    }

    #[test]
    fn secret_store_lifecycle() {
        let s = state();
        assert!(!s.has_key());
        s.set_key("sk-test-123").unwrap();
        assert!(s.has_key());
        assert_eq!(s.key().unwrap().as_deref(), Some("sk-test-123"));
        s.delete_key().unwrap();
        assert!(!s.has_key());
        // 删除不存在的 key 不报错。
        s.delete_key().unwrap();
    }

    #[test]
    fn serde_defaults_fill_missing_fields() {
        let cfg: AiConfig = serde_json::from_str(r#"{"enabled":true}"#).unwrap();
        assert!(cfg.enabled);
        assert_eq!(cfg.provider, ProviderKind::Ollama);
        assert!(cfg.exclude_patterns.len() > 5);
    }
}
