//! AI 助手命令（P11，ADR-013）：配置（key 不出 Rust）、发送前预览、
//! 智能提交消息与日报/周报生成（Task 化 + 流式事件 + 可取消）。
//!
//! - 命令命名走 `ai_<动作>`（PLAN §4.2），specta 登记；
//! - 生成任务：`ai_generate_*` 返回 taskId，`AiEvent` 事件推送
//!   `status | delta | done | cancelled | failed`（与 clone 进度同构）；
//! - `ai_cancel` 经 CancelToken 中断（SSE 流循环内即时生效）；
//! - 隐私红线：`ai_config_get` 只回 `has_key` 布尔位，无任何接口回明文。

use crate::core::ai::config::{AiConfig, ProviderKind};
use crate::core::ai::context::{self, PathFilter};
use crate::core::ai::generate::{self, OnStatus};
use crate::core::error::AppError;
use crate::core::repo::{RepoId, RepoManager};
use crate::core::runner::CancelToken;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{Manager, State};
use tokio::sync::Mutex as AsyncMutex;

use super::net::AppEmitter;

// =====================
// 类型
// =====================

/// 前端可见的配置视图（非敏感；`has_key` 只是存在性）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, specta::Type)]
pub struct AiConfigDto {
    pub enabled: bool,
    pub provider: ProviderKind,
    pub base_url: String,
    pub model: String,
    pub timeout_secs: u32,
    pub conventional: bool,
    pub custom_auth: String,
    pub exclude_patterns: Vec<String>,
    pub excluded_repos: Vec<String>,
    pub has_key: bool,
}

impl AiConfigDto {
    fn from(cfg: AiConfig, has_key: bool) -> Self {
        Self {
            enabled: cfg.enabled,
            provider: cfg.provider,
            base_url: cfg.base_url,
            model: cfg.model,
            timeout_secs: cfg.timeout_secs,
            conventional: cfg.conventional,
            custom_auth: cfg.custom_auth,
            exclude_patterns: cfg.exclude_patterns,
            excluded_repos: cfg.excluded_repos,
            has_key,
        }
    }

    fn into_config(self) -> AiConfig {
        AiConfig {
            enabled: self.enabled,
            provider: self.provider,
            base_url: self.base_url,
            model: self.model,
            timeout_secs: self.timeout_secs,
            conventional: self.conventional,
            custom_auth: self.custom_auth,
            exclude_patterns: self.exclude_patterns,
            excluded_repos: self.excluded_repos,
        }
    }
}

/// 日报/周报生成请求。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct AiReportRequest {
    /// `daily` | `weekly`
    pub kind: String,
    /// author email 过滤；空 = 全部成员。
    pub author: Option<String>,
    /// 聚合的仓库（当前 + 手动勾选，PLAN P11 跨仓库 v1）。
    pub repo_ids: Vec<RepoId>,
    /// 输出语言（`zh-CN` / `en`，跟随前端 locale）。
    pub language: String,
}

/// 发送前预览（ADR-013 隐私红线：先看再发）。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct AiPreview {
    /// `commit_message` | `report`
    pub kind: String,
    pub provider: String,
    /// 文件清单（提交消息）或各仓库提交数（报告）。
    pub items: Vec<String>,
    pub commits: u32,
    pub chars: u32,
    pub excluded: u32,
    pub truncated: bool,
    /// map-reduce 批次（1 = 单次请求）。
    pub batches: u32,
}

/// 生成任务事件（与 CloneEvent 同构的事件流）。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, tauri_specta::Event)]
pub struct AiEvent {
    pub task_id: u32,
    /// `status` | `delta` | `done` | `cancelled` | `failed`
    pub phase: String,
    pub text: Option<String>,
    pub message: Option<String>,
    pub error: Option<AppError>,
}

/// AI 生成任务注册表：taskId → CancelToken。
#[derive(Default)]
pub struct AiTasks(Arc<AsyncMutex<HashMap<u32, CancelToken>>>);

static NEXT_AI_TASK: AtomicU32 = AtomicU32::new(1);

fn next_task_id() -> u32 {
    NEXT_AI_TASK.fetch_add(1, Ordering::Relaxed)
}

fn emit_ai(app: &tauri::AppHandle, event: AiEvent) {
    use tauri_specta::Event as _;
    let _ = event.emit(app);
}

fn status_emitter(task_id: u32, app: tauri::AppHandle) -> OnStatus {
    Arc::new(move |message: String| {
        emit_ai(
            &app,
            AiEvent {
                task_id,
                phase: "status".into(),
                text: None,
                message: Some(message),
                error: None,
            },
        );
    })
}

/// 增量节流发射器：≥24 字符或 120ms 才发一次 delta，摊薄 IPC 消息量。
struct ThrottledDelta {
    task_id: u32,
    app: tauri::AppHandle,
    buf: String,
    last: Instant,
}

impl ThrottledDelta {
    fn new(task_id: u32, app: tauri::AppHandle) -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self {
            task_id,
            app,
            buf: String::new(),
            last: Instant::now(),
        }))
    }

    fn callback(this: Arc<Mutex<Self>>) -> generate::OnDelta {
        Arc::new(move |delta: &str| {
            let mut st = this.lock().unwrap();
            st.buf.push_str(delta);
            if st.buf.chars().count() >= 24 || st.last.elapsed() >= Duration::from_millis(120) {
                st.flush();
            }
        })
    }

    fn flush(&mut self) {
        if self.buf.is_empty() {
            return;
        }
        let chunk = std::mem::take(&mut self.buf);
        self.last = Instant::now();
        emit_ai(
            &self.app,
            AiEvent {
                task_id: self.task_id,
                phase: "delta".into(),
                text: Some(chunk),
                message: None,
                error: None,
            },
        );
    }
}

/// 仓库级排除判断：路径规范化后比较（Windows 大小写/分隔符差异归一）。
fn is_repo_excluded(cfg: &AiConfig, repo_path: &str) -> bool {
    if cfg.excluded_repos.is_empty() {
        return false;
    }
    let norm = |p: &str| {
        std::fs::canonicalize(p)
            .map(|c| c.display().to_string().to_lowercase())
            .unwrap_or_else(|_| p.to_lowercase().replace('/', "\\"))
    };
    let target = norm(repo_path);
    cfg.excluded_repos.iter().any(|r| norm(r) == target)
}

fn guard_enabled(cfg: &AiConfig) -> Result<(), AppError> {
    if cfg.enabled {
        Ok(())
    } else {
        Err(AppError::AiDisabled)
    }
}

// =====================
// 配置命令
// =====================

#[tauri::command]
#[specta::specta]
pub async fn ai_config_get(
    state: State<'_, crate::core::ai::AiState>,
) -> Result<AiConfigDto, AppError> {
    Ok(AiConfigDto::from(state.load_config(), state.has_key()))
}

#[tauri::command]
#[specta::specta]
pub async fn ai_config_set(
    config: AiConfigDto,
    state: State<'_, crate::core::ai::AiState>,
) -> Result<AiConfigDto, AppError> {
    let mut dto = config;
    dto.has_key = state.has_key(); // key 不随此命令变更
    dto.timeout_secs = dto.timeout_secs.clamp(5, 600);
    let cfg = dto.clone().into_config();
    state.save_config(&cfg)?;
    Ok(dto)
}

#[tauri::command]
#[specta::specta]
pub async fn ai_set_key(
    key: String,
    state: State<'_, crate::core::ai::AiState>,
) -> Result<(), AppError> {
    let key = key.trim().to_string();
    if key.is_empty() {
        return Err(AppError::parse("api key is empty"));
    }
    state.set_key(&key)
}

#[tauri::command]
#[specta::specta]
pub async fn ai_delete_key(state: State<'_, crate::core::ai::AiState>) -> Result<(), AppError> {
    state.delete_key()
}

/// 连通性测试：用当前配置发一个最小请求，返回 provider 标签 + 模型应答。
#[tauri::command]
#[specta::specta]
pub async fn ai_test_connection(
    state: State<'_, crate::core::ai::AiState>,
) -> Result<String, AppError> {
    let cfg = state.load_config();
    guard_enabled(&cfg)?;
    let key = state.key()?;
    let provider = generate::build_provider(&cfg, key, &state.net)?;
    let label = provider.label();
    let text = provider
        .stream_complete(
            generate::CompletionRequest {
                system: "You are a connectivity probe. Reply with exactly: OK".into(),
                user: "ping".into(),
                max_tokens: 16,
            },
            None,
            Arc::new(|_| {}),
        )
        .await?;
    Ok(format!("{label} ✓ {text}"))
}

// =====================
// 发送前预览
// =====================

#[tauri::command]
#[specta::specta]
pub async fn ai_preview_commit_message(
    repo_id: RepoId,
    repos: State<'_, RepoManager>,
    state: State<'_, crate::core::ai::AiState>,
) -> Result<AiPreview, AppError> {
    let cfg = state.load_config();
    guard_enabled(&cfg)?;
    let repo_path = resolve_repo(&repos, repo_id).await?;
    if is_repo_excluded(&cfg, repo_path.1.as_str()) {
        return Err(AppError::ai_config(
            "this repository is excluded from AI features (Settings → AI)",
        ));
    }
    let model = generate::collect_staged(repos.engine().as_ref(), &repos, &repo_path.1).await?;
    if model.files.is_empty() {
        return Err(AppError::parse("nothing staged"));
    }
    let filter = PathFilter::new(&cfg.exclude_patterns);
    let ctx = context::build_diff_context(&model, &filter, &context::DiffBudget::default());
    Ok(AiPreview {
        kind: "commit_message".into(),
        provider: generate::provider_label(&cfg),
        items: ctx
            .files
            .iter()
            .map(|f| {
                if f.is_binary {
                    format!("{} (binary)", f.path)
                } else {
                    format!("{} (+{} -{})", f.path, f.added, f.removed)
                }
            })
            .collect(),
        commits: 0,
        chars: ctx.total_chars as u32,
        excluded: ctx.excluded_files as u32,
        truncated: ctx.truncated,
        batches: 1,
    })
}

#[tauri::command]
#[specta::specta]
pub async fn ai_preview_report(
    request: AiReportRequest,
    repos: State<'_, RepoManager>,
    state: State<'_, crate::core::ai::AiState>,
) -> Result<AiPreview, AppError> {
    let cfg = state.load_config();
    guard_enabled(&cfg)?;
    let kind = validate_kind(&request.kind)?;
    let sources = resolve_sources(&repos, &request.repo_ids, &cfg).await?;
    if sources.is_empty() {
        return Err(AppError::parse("no repositories selected"));
    }
    let since = generate::report_range(&kind);
    let author = request
        .author
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let commits = generate::collect_report(
        repos.engine().as_ref(),
        &repos,
        &sources,
        &since,
        author,
        None,
        Arc::new(|_| {}),
    )
    .await?;
    let filter = PathFilter::new(&cfg.exclude_patterns);
    let (text, excluded) = context::render_report_batch(&commits, &filter);
    let batches = if context::needs_map_reduce(&commits, &filter) {
        context::batch_for_map_reduce(
            &commits,
            &filter,
            context::BATCH_MAX_COMMITS,
            context::BATCH_MAX_CHARS,
        )
        .len() as u32
    } else {
        1
    };
    // 按仓库统计提交数。
    let mut per_repo: HashMap<String, u32> = HashMap::new();
    for c in &commits {
        *per_repo.entry(c.repo.clone()).or_default() += 1;
    }
    let items = sources
        .iter()
        .map(|(label, _)| format!("{label}: {}", per_repo.get(label).copied().unwrap_or(0)))
        .collect();
    Ok(AiPreview {
        kind: "report".into(),
        provider: generate::provider_label(&cfg),
        items,
        commits: commits.len() as u32,
        chars: text.chars().count() as u32,
        excluded: excluded as u32,
        truncated: false,
        batches,
    })
}

// =====================
// 生成任务
// =====================

#[tauri::command]
#[specta::specta]
pub async fn ai_generate_commit_message(
    repo_id: RepoId,
    language: String,
    repos: State<'_, RepoManager>,
    state: State<'_, crate::core::ai::AiState>,
    emitter: State<'_, AppEmitter>,
    tasks: State<'_, AiTasks>,
) -> Result<u32, AppError> {
    let cfg = state.load_config();
    guard_enabled(&cfg)?;
    let (_id, repo_path) = resolve_repo(&repos, repo_id).await?;
    if is_repo_excluded(&cfg, &repo_path) {
        return Err(AppError::ai_config(
            "this repository is excluded from AI features (Settings → AI)",
        ));
    }
    let key = state.key()?;
    let engine = repos.engine();
    let net = state.net.clone();
    let app = emitter.inner().0.clone();
    let registry = tasks.inner().0.clone();

    let task_id = next_task_id();
    let token = CancelToken::new();
    registry.lock().await.insert(task_id, token.clone());

    tauri::async_runtime::spawn(async move {
        let emit_status = status_emitter(task_id, app.clone());
        let throttled = ThrottledDelta::new(task_id, app.clone());
        let result: Result<String, AppError> = async {
            let provider = generate::build_provider(&cfg, key, &net)?;
            let repos_state = app.state::<RepoManager>();
            emit_status("collecting staged diff…".into());
            let model = generate::collect_staged(engine.as_ref(), &repos_state, &repo_path).await?;
            let filter = PathFilter::new(&cfg.exclude_patterns);
            generate::generate_commit_message(
                provider.as_ref(),
                &model,
                &filter,
                &cfg,
                &language,
                Some(&token),
                ThrottledDelta::callback(throttled.clone()),
            )
            .await
        }
        .await;
        throttled.lock().unwrap().flush();
        registry.lock().await.remove(&task_id);
        finish_event(&app, task_id, result, "commit message");
    });

    Ok(task_id)
}

#[tauri::command]
#[specta::specta]
pub async fn ai_generate_report(
    request: AiReportRequest,
    repos: State<'_, RepoManager>,
    state: State<'_, crate::core::ai::AiState>,
    emitter: State<'_, AppEmitter>,
    tasks: State<'_, AiTasks>,
) -> Result<u32, AppError> {
    let cfg = state.load_config();
    guard_enabled(&cfg)?;
    let kind = validate_kind(&request.kind)?;
    let sources = resolve_sources(&repos, &request.repo_ids, &cfg).await?;
    if sources.is_empty() {
        return Err(AppError::parse("no repositories selected"));
    }
    let since = generate::report_range(&kind);
    let author = request
        .author
        .clone()
        .map(|a| a.trim().to_string())
        .filter(|a| !a.is_empty());
    let author_note = match author.as_deref() {
        Some(email) => format!("Only commits authored by <{email}> are included."),
        None => "Commits from all authors are included.".to_string(),
    };
    let language = request.language;
    let key = state.key()?;
    let engine = repos.engine();
    let net = state.net.clone();
    let app = emitter.inner().0.clone();
    let registry = tasks.inner().0.clone();

    let task_id = next_task_id();
    let token = CancelToken::new();
    registry.lock().await.insert(task_id, token.clone());

    tauri::async_runtime::spawn(async move {
        let emit_status = status_emitter(task_id, app.clone());
        let throttled = ThrottledDelta::new(task_id, app.clone());
        let result: Result<String, AppError> = async {
            let provider = generate::build_provider(&cfg, key, &net)?;
            let repos_state = app.state::<RepoManager>();
            emit_status(format!("collecting {} repositories…", sources.len()));
            let commits = generate::collect_report(
                engine.as_ref(),
                &repos_state,
                &sources,
                &since,
                author.as_deref(),
                Some(&token),
                emit_status.clone(),
            )
            .await?;
            let filter = PathFilter::new(&cfg.exclude_patterns);
            generate::generate_report(
                provider.as_ref(),
                &commits,
                &filter,
                &generate::ReportParams {
                    kind,
                    since,
                    repo_count: sources.len(),
                    author_note,
                    language,
                },
                Some(&token),
                ThrottledDelta::callback(throttled.clone()),
                emit_status,
            )
            .await
        }
        .await;
        throttled.lock().unwrap().flush();
        registry.lock().await.remove(&task_id);
        finish_event(&app, task_id, result, "report");
    });

    Ok(task_id)
}

#[tauri::command]
#[specta::specta]
pub async fn ai_cancel(task_id: u32, tasks: State<'_, AiTasks>) -> Result<(), AppError> {
    let token = tasks.0.lock().await.remove(&task_id);
    match token {
        Some(t) => {
            t.cancel();
            Ok(())
        }
        None => Err(AppError::parse(format!("unknown ai task {task_id}"))),
    }
}

/// 导出报告为 .md（路径来自前端 save 对话框，内容为已生成的文本）。
#[tauri::command]
#[specta::specta]
pub async fn ai_export_markdown(path: String, content: String) -> Result<(), AppError> {
    let path = path.trim();
    if path.is_empty() {
        return Err(AppError::parse("export path is empty"));
    }
    std::fs::write(path, content.as_bytes())
        .map_err(|e| AppError::io_with_detail(e.to_string(), "export markdown"))?;
    Ok(())
}

// =====================
// 辅助
// =====================

/// 解析仓库 id → (展示标签, 根路径)。
async fn resolve_repo(
    repos: &State<'_, RepoManager>,
    id: RepoId,
) -> Result<(String, String), AppError> {
    let path = repos.get_path(id).await.ok_or(AppError::InvalidRepo {
        path: id.0.to_string(),
    })?;
    let label = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    Ok((label, path.display().to_string()))
}

/// 解析报告数据源（排除仓库级排除项）。
async fn resolve_sources(
    repos: &State<'_, RepoManager>,
    ids: &[RepoId],
    cfg: &AiConfig,
) -> Result<Vec<(String, String)>, AppError> {
    let mut out = Vec::new();
    for id in ids {
        let (label, path) = resolve_repo(repos, *id).await?;
        if is_repo_excluded(cfg, &path) {
            continue;
        }
        out.push((label, path));
    }
    Ok(out)
}

fn validate_kind(kind: &str) -> Result<String, AppError> {
    match kind {
        "daily" | "weekly" => Ok(kind.to_string()),
        other => Err(AppError::parse(format!("unknown report kind: {other}"))),
    }
}

/// 终态事件：done / cancelled / failed。
fn finish_event(
    app: &tauri::AppHandle,
    task_id: u32,
    result: Result<String, AppError>,
    what: &str,
) {
    match result {
        Ok(text) => emit_ai(
            app,
            AiEvent {
                task_id,
                phase: "done".into(),
                text: Some(text),
                message: None,
                error: None,
            },
        ),
        Err(AppError::OperationCancelled) => emit_ai(
            app,
            AiEvent {
                task_id,
                phase: "cancelled".into(),
                text: None,
                message: Some(format!("{what} cancelled")),
                error: None,
            },
        ),
        Err(e) => {
            tracing::warn!(task_id, what, "ai generation failed: {e}");
            emit_ai(
                app,
                AiEvent {
                    task_id,
                    phase: "failed".into(),
                    text: None,
                    message: None,
                    error: Some(e),
                },
            );
        }
    }
}
