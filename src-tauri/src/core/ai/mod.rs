//! AI Provider 管线（P11，ADR-013）。
//!
//! 布局：
//! - [`config`]：非敏感配置（`{appData}/ai/config.json`）+ keyring 密钥；
//! - [`sse`]：SSE 增量解析器；
//! - [`provider`]：Provider trait + OpenAI 兼容 / Anthropic / 自定义 HTTP；
//! - [`context`]：隐私过滤（glob 排除）、diff 采样、报告上下文与分批；
//! - [`prompt`]：提示词；
//! - [`generate`]：生成编排（采集 → 构建 → 流式 → map-reduce）。
//!
//! 红线（ADR-013）：key 只在 Rust 侧与 OS keychain；默认 opt-in 关闭；
//! 发送前预览；上下文最小化（报告不发 diff）。

pub mod config;
pub mod context;
pub mod generate;
pub mod prompt;
pub mod provider;
pub mod sse;

pub use config::{AiConfig, AiState, ProviderKind};
pub use provider::AiProvider;
