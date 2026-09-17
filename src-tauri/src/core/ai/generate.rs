//! 生成编排（P11）：数据采集（GitEngine）→ 上下文 → Provider 流式补全。
//!
//! 与 Tauri 解耦：引擎经 `&dyn GitEngine` 注入、采集配额经 `&RepoManager`
//! 取 `read_permit`（全局 ≤8），wiremock 集成测试直接驱动本模块。

use crate::core::ai::config::{AiConfig, ProviderKind};
use crate::core::ai::context::{
    self, batch_for_map_reduce, build_diff_context, needs_map_reduce, render_report_batch,
    DiffBudget, PathFilter,
};
use crate::core::ai::prompt;
// pub use：commands 层直接引用 OnDelta / CompletionRequest。
pub use crate::core::ai::provider::{AiProvider, AuthStyle, CompletionRequest, OnDelta};
use crate::core::engine::{DiffModel, DiffSource, GitEngine, NumstatCommit};
use crate::core::error::AppError;
use crate::core::repo::RepoManager;
use crate::core::runner::CancelToken;
use chrono::Datelike;
use std::sync::Arc;

/// 状态/阶段通知（事件层转发给前端）。
pub type OnStatus = Arc<dyn Fn(String) + Send + Sync>;

// =====================
// Provider 构建
// =====================

/// 从配置 + key 构建 provider；配置不完整返回 `AiConfig`（P11 验收：
/// 明确报错而非网络层困惑）。
pub fn build_provider(
    cfg: &AiConfig,
    key: Option<String>,
    net: &crate::core::runner::SharedNetConfig,
) -> Result<Box<dyn AiProvider>, AppError> {
    if cfg.model.trim().is_empty() {
        return Err(AppError::ai_config(
            "model is not configured (Settings → AI)",
        ));
    }
    let key = key.filter(|k| !k.trim().is_empty());
    let label = provider_label(cfg);
    let timeout = cfg.timeout_secs.max(5);
    match cfg.provider {
        ProviderKind::Ollama => {
            let base = cfg.base_url.trim().trim_end_matches('/');
            if base.is_empty() {
                return Err(AppError::ai_config("Base URL is empty"));
            }
            Ok(Box::new(crate::core::ai::provider::OpenAiProvider::new(
                crate::core::ai::provider::OpenAiProvider::completions_url(base),
                String::new(),
                AuthStyle::None,
                cfg.model.trim().to_string(),
                net,
                timeout,
                label,
            )))
        }
        ProviderKind::OpenAiCompatible => {
            let base = cfg.base_url.trim().trim_end_matches('/');
            if base.is_empty() {
                return Err(AppError::ai_config("Base URL is empty"));
            }
            if !base.starts_with("http://") && !base.starts_with("https://") {
                return Err(AppError::ai_config(format!("invalid Base URL: {base}")));
            }
            let Some(key) = key else {
                return Err(AppError::ai_config(
                    "API key is missing (Settings → AI → 保存密钥)",
                ));
            };
            Ok(Box::new(crate::core::ai::provider::OpenAiProvider::new(
                crate::core::ai::provider::OpenAiProvider::completions_url(base),
                key,
                AuthStyle::Bearer,
                cfg.model.trim().to_string(),
                net,
                timeout,
                label,
            )))
        }
        ProviderKind::Anthropic => {
            let base = cfg.base_url.trim().trim_end_matches('/');
            if base.is_empty() {
                return Err(AppError::ai_config("Base URL is empty"));
            }
            if !base.starts_with("http://") && !base.starts_with("https://") {
                return Err(AppError::ai_config(format!("invalid Base URL: {base}")));
            }
            let Some(key) = key else {
                return Err(AppError::ai_config(
                    "API key is missing (Settings → AI → 保存密钥)",
                ));
            };
            Ok(Box::new(crate::core::ai::provider::AnthropicProvider::new(
                base.to_string(),
                key,
                cfg.model.trim().to_string(),
                net,
                timeout,
                label,
            )))
        }
        ProviderKind::Custom => {
            let url = cfg.base_url.trim().trim_end_matches('/');
            if !url.starts_with("http://") && !url.starts_with("https://") {
                return Err(AppError::ai_config(
                    "custom endpoint must be a full http(s) URL",
                ));
            }
            let auth = match cfg.custom_auth.as_str() {
                "x_api_key" => AuthStyle::XApiKey,
                "none" => AuthStyle::None,
                _ => AuthStyle::Bearer,
            };
            if auth != AuthStyle::None && key.is_none() {
                return Err(AppError::ai_config(
                    "auth style requires an API key (or switch to none)",
                ));
            }
            Ok(Box::new(crate::core::ai::provider::OpenAiProvider::new(
                url.to_string(),
                key.unwrap_or_default(),
                auth,
                cfg.model.trim().to_string(),
                net,
                timeout,
                label,
            )))
        }
    }
}

/// 预览/事件里的 provider 展示名（Ollama 标注本地离线）。
pub fn provider_label(cfg: &AiConfig) -> String {
    let model = cfg.model.trim();
    let host = host_of(&cfg.base_url);
    match cfg.provider {
        ProviderKind::Ollama => format!("Ollama（本地离线）· {model}"),
        ProviderKind::OpenAiCompatible => format!("OpenAI 兼容 · {host} · {model}"),
        ProviderKind::Anthropic => format!("Anthropic · {model}"),
        ProviderKind::Custom => format!("自定义端点 · {host} · {model}"),
    }
}

fn host_of(url: &str) -> &str {
    url.trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split(['/', ':'])
        .next()
        .unwrap_or(url)
}

// =====================
// 智能提交消息
// =====================

/// 采集 staged diff（read permit 短持有）。
pub async fn collect_staged(
    engine: &dyn GitEngine,
    repos: &RepoManager,
    repo_path: &str,
) -> Result<DiffModel, AppError> {
    let _permit = repos.read_permit().await?;
    let model = engine
        .diff(repo_path, DiffSource::Staged, None, &[], Default::default())
        .await?;
    Ok(model)
}

/// 生成提交消息（流式）。返回完整消息文本。
pub async fn generate_commit_message(
    provider: &dyn AiProvider,
    model: &DiffModel,
    filter: &PathFilter,
    cfg: &AiConfig,
    language: &str,
    cancel: Option<&CancelToken>,
    on_delta: OnDelta,
) -> Result<String, AppError> {
    if model.files.is_empty() {
        return Err(AppError::parse(
            "nothing staged: stage some changes before generating a commit message",
        ));
    }
    let ctx = build_diff_context(model, filter, &DiffBudget::default());
    let req = CompletionRequest {
        system: prompt::commit_message_system(cfg.conventional, language),
        user: prompt::commit_message_user(&ctx),
        max_tokens: 500,
    };
    provider.stream_complete(req, cancel, on_delta).await
}

// =====================
// 日报 / 周报
// =====================

/// 报告范围（PLAN P11）：日报 = 今天 00:00 起；周报 = 本周一 00:00 起
/// （本地时区，git `--since` 直接解析 ISO 日期时间）。
pub fn report_range(kind: &str) -> String {
    let now = chrono::Local::now();
    let date = match kind {
        "weekly" => {
            let days_from_monday = now.weekday().num_days_from_monday() as i64;
            now.date_naive() - chrono::Duration::days(days_from_monday)
        }
        _ => now.date_naive(),
    };
    format!("{date}T00:00:00")
}

/// 采集报告数据：逐仓库 `log --all --numstat --since/--author`（read permit
/// 短持有），commit 打上仓库标签。`repos` = (展示标签, 仓库根路径)。
pub async fn collect_report(
    engine: &dyn GitEngine,
    repos: &RepoManager,
    sources: &[(String, String)],
    since: &str,
    author: Option<&str>,
    cancel: Option<&CancelToken>,
    on_status: OnStatus,
) -> Result<Vec<NumstatCommit>, AppError> {
    const PER_REPO_LIMIT: u32 = 2000;
    let mut all = Vec::new();
    for (label, path) in sources {
        if cancel.is_some_and(|t| t.is_cancelled()) {
            return Err(AppError::OperationCancelled);
        }
        on_status(format!("collecting {label}…"));
        let _permit = repos.read_permit().await?;
        let mut commits = engine
            .log_numstat(path, Some(since), None, author, PER_REPO_LIMIT)
            .await?;
        for c in &mut commits {
            c.repo = label.clone();
        }
        all.extend(commits);
    }
    // 时间倒序 → map/reduce 批次按时间近邻成组。
    all.sort_by(|a, b| b.date.cmp(&a.date));
    Ok(all)
}

/// 报告生成的展示参数（收敛 generate_report 的参数量）。
pub struct ReportParams {
    pub kind: String,
    pub since: String,
    pub repo_count: usize,
    pub author_note: String,
    pub language: String,
}

/// 报告生成（流式）：小范围单次请求；大范围 map-reduce
/// （分批摘要，并发 ≤2，再汇总）。返回完整 Markdown。
pub async fn generate_report(
    provider: &dyn AiProvider,
    commits: &[NumstatCommit],
    filter: &PathFilter,
    params: &ReportParams,
    cancel: Option<&CancelToken>,
    on_delta: OnDelta,
    on_status: OnStatus,
) -> Result<String, AppError> {
    if commits.is_empty() {
        return Err(AppError::parse(
            "no commits found in the selected range (nothing to report)",
        ));
    }

    if !needs_map_reduce(commits, filter) {
        on_status(format!("generating report from {} commits…", commits.len()));
        let (ctx_text, _) = render_report_batch(commits, filter);
        let req = CompletionRequest {
            system: prompt::report_system(&params.language),
            user: prompt::report_user(
                &params.kind,
                &params.since,
                params.repo_count,
                &params.author_note,
                &ctx_text,
            ),
            max_tokens: 4000,
        };
        return provider.stream_complete(req, cancel, on_delta).await;
    }

    // ---- map：分批摘要（并发 ≤2，取消即时中断）----
    let batches = batch_for_map_reduce(
        commits,
        filter,
        context::BATCH_MAX_COMMITS,
        context::BATCH_MAX_CHARS,
    );
    let total = batches.len();
    on_status(format!(
        "{} commits → {} batches, summarizing…",
        commits.len(),
        total
    ));

    // join_all 借用 provider/commits（不 spawn），信号量限并发 ≤2；
    // 取消令牌透传到每个请求，select 在流循环内即时生效。
    let sem = Arc::new(tokio::sync::Semaphore::new(2));
    let mut futs = Vec::with_capacity(total);
    for (i, batch) in batches.iter().enumerate() {
        let batch_commits: Vec<&NumstatCommit> = batch.iter().map(|&idx| &commits[idx]).collect();
        let (ctx_text, _) = context::render_report_batch_refs(&batch_commits, filter);
        let req = CompletionRequest {
            system: prompt::map_system(&params.language),
            user: prompt::map_user(&ctx_text),
            max_tokens: 1200,
        };
        let sem = sem.clone();
        let status = on_status.clone();
        futs.push(async move {
            let _permit = sem.acquire_owned().await;
            status(format!("summarizing batch {}/{}", i + 1, total));
            (
                i,
                provider
                    .stream_complete(req, cancel, Arc::new(|_: &str| {}))
                    .await,
            )
        });
    }
    let results = futures_util::future::join_all(futs).await;

    let mut results: Vec<(usize, Result<String, AppError>)> = results;
    results.sort_by_key(|(i, _)| *i);
    let mut summaries = Vec::with_capacity(total);
    for (i, r) in results {
        match r {
            Ok(s) if !s.trim().is_empty() => summaries.push(s),
            Ok(_) => {
                return Err(AppError::AiProvider {
                    status: 200,
                    message: format!("map batch {} returned an empty summary", i + 1),
                })
            }
            Err(AppError::OperationCancelled) => return Err(AppError::OperationCancelled),
            Err(e) => return Err(e),
        }
    }

    // ---- reduce：汇总成文 ----
    on_status("merging summaries…".to_string());
    let req = CompletionRequest {
        system: prompt::reduce_system(&params.language),
        user: prompt::reduce_user(&params.kind, &params.since, params.repo_count, &summaries),
        max_tokens: 4000,
    };
    provider.stream_complete(req, cancel, on_delta).await
}
