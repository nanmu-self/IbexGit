//! AI Provider 管线（P11，ADR-013）：Rust 侧直连用户端点，BYOK。
//!
//! - **OpenAI 兼容**（`POST {base}/chat/completions`，SSE `choices[0].delta.content`）
//!   覆盖 OpenAI / Ollama OpenAI 兼容层 / DeepSeek / Qwen / Moonshot / vLLM /
//!   LM Studio；**Custom** = 同一 wire format + 完整 URL + 可选鉴权头风格；
//! - **Anthropic**（`POST {base}/v1/messages`，SSE `content_block_delta`）；
//! - reqwest（rustls + stream）；代理策略继承 P7 `SharedNetConfig`
//!   （inherit = 读取环境变量，none = no_proxy，custom = 显式代理）；
//! - 取消：流式读取循环 `select!` CancelToken，取消即丢弃连接；
//! - 错误归一化为 `AppError` 的 `AiNetwork / AiAuth / AiRateLimit / AiProvider`
//!   变体（P11 验收：断网 / 离线 / 超时 / 额度均有明确反馈）。

use crate::core::ai::sse::SseParser;
use crate::core::error::AppError;
use crate::core::runner::{CancelToken, ProxyMode, SharedNetConfig};
use async_trait::async_trait;
use futures_util::StreamExt;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

/// 一次补全请求（Rust 侧构建提示词，key 不经此路径进出 WebView）。
#[derive(Debug, Clone)]
pub struct CompletionRequest {
    pub system: String,
    pub user: String,
    /// 输出上限（tokens 粗估由 provider 解释）。
    pub max_tokens: u32,
}

pub type OnDelta = Arc<dyn Fn(&str) + Send + Sync>;

#[async_trait]
pub trait AiProvider: Send + Sync {
    /// 展示名（预览 / 错误信息用），如 `Ollama (qwen3:8b)`。
    fn label(&self) -> String;

    /// 流式补全：增量文本经 `on_delta` 推送；返回完整文本。
    /// 取消语义：中途取消立即返回 `OperationCancelled`，连接随即丢弃。
    async fn stream_complete(
        &self,
        req: CompletionRequest,
        cancel: Option<&CancelToken>,
        on_delta: OnDelta,
    ) -> Result<String, AppError>;
}

// =====================
// HTTP 客户端构建（代理继承 P7）
// =====================

/// 按共享网络配置构建 reqwest 客户端。`total_timeout` 仅用于一次性请求
/// （连通性测试）；流式请求改用**逐块空闲超时**（长生成不会被总超时截断）。
pub fn build_client(net: &SharedNetConfig, total_timeout: Option<Duration>) -> reqwest::Client {
    let snap = net.snapshot();
    let mut builder = reqwest::Client::builder().connect_timeout(Duration::from_secs(10));
    if let Some(t) = total_timeout {
        builder = builder.timeout(t);
    }
    match snap.proxy_mode {
        ProxyMode::Inherit => {} // reqwest 默认读取 http_proxy/https_proxy 环境变量
        ProxyMode::None => builder = builder.no_proxy(),
        ProxyMode::Custom => {
            if let Some(url) = snap.proxy_url.as_deref().filter(|s| !s.is_empty()) {
                if let Ok(p) = reqwest::Proxy::all(url) {
                    builder = builder.proxy(p);
                }
            }
        }
    }
    builder.build().unwrap_or_default()
}

// =====================
// HTTP 错误归一化
// =====================

/// 把 HTTP 状态码 + 响应体映射为 AppError（区分认证 / 额度 / 其他）。
fn map_http_error(status: u16, body: &str) -> AppError {
    // 尽力从 JSON 里抠 error.message（OpenAI / Anthropic 同构）。
    let detail = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.pointer("/error/message")
                .and_then(|m| m.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| body.chars().take(300).collect());
    match status {
        401 | 403 => AppError::AiAuth {
            status: status as u32,
            message: detail,
        },
        429 => AppError::AiRateLimit {
            status: status as u32,
            message: detail,
        },
        _ => AppError::AiProvider {
            status: status as u32,
            message: detail,
        },
    }
}

/// reqwest 传输层错误 → AiNetwork（断网 / DNS / 连接拒绝 / 空闲超时）。
fn map_transport_error(e: reqwest::Error) -> AppError {
    let kind = if e.is_timeout() {
        "timed out"
    } else if e.is_connect() {
        "connection failed"
    } else {
        "request failed"
    };
    AppError::ai_network(format!("{kind}: {e}"))
}

// =====================
// OpenAI 兼容（含 Ollama / DeepSeek / Qwen / Custom）
// =====================

/// 鉴权头风格（Custom provider 用；内置类型自动决定）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthStyle {
    /// `Authorization: Bearer <key>`（OpenAI 兼容缺省）。
    Bearer,
    /// `x-api-key: <key>`。
    XApiKey,
    /// 不带鉴权头（本地 Ollama / 无鉴权网关）。
    None,
}

pub struct OpenAiProvider {
    /// 完整端点 URL（构建时已拼接好）。
    url: String,
    key: String,
    auth: AuthStyle,
    model: String,
    client: reqwest::Client,
    /// 流式读取的空闲超时。
    idle_timeout: Duration,
    label: String,
}

impl OpenAiProvider {
    pub fn new(
        url: String,
        key: String,
        auth: AuthStyle,
        model: String,
        net: &SharedNetConfig,
        timeout_secs: u32,
        label: String,
    ) -> Self {
        Self {
            url,
            key,
            auth,
            model,
            client: build_client(net, None),
            idle_timeout: Duration::from_secs(timeout_secs.max(1) as u64),
            label,
        }
    }

    /// 从端点根 URL 拼接 chat/completions 路径（`Custom` 直接传完整 URL）。
    pub fn completions_url(base: &str) -> String {
        base.trim_end_matches('/').to_string() + "/chat/completions"
    }

    async fn send(
        &self,
        req: CompletionRequest,
        cancel: Option<&CancelToken>,
        on_delta: OnDelta,
    ) -> Result<String, AppError> {
        let body = json!({
            "model": self.model,
            "messages": [
                {"role": "system", "content": req.system},
                {"role": "user", "content": req.user},
            ],
            "stream": true,
            "max_tokens": req.max_tokens,
            "temperature": 0.4,
        });
        let mut http = self
            .client
            .post(&self.url)
            .header("content-type", "application/json");
        match self.auth {
            AuthStyle::Bearer if !self.key.is_empty() => {
                http = http.bearer_auth(&self.key);
            }
            AuthStyle::XApiKey if !self.key.is_empty() => {
                http = http.header("x-api-key", &self.key);
            }
            _ => {}
        }

        // 取消在请求建立阶段同样生效（用户取消时立即返回，不等服务端响应）。
        let request_fut = http.body(body.to_string()).send();
        let response = match cancel {
            Some(token) => tokio::select! {
                biased;
                _ = token.cancelled() => return Err(AppError::OperationCancelled),
                resp = request_fut => resp.map_err(map_transport_error)?,
            },
            None => request_fut.await.map_err(map_transport_error)?,
        };
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(map_http_error(status.as_u16(), &body));
        }

        stream_sse(response, self.idle_timeout, cancel, on_delta, |data| {
            // `data: [DONE]` 终止；其余取 choices[0].delta.content。
            if data == "[DONE]" {
                return Ok(None);
            }
            let v: serde_json::Value = serde_json::from_str(data)
                .map_err(|e| AppError::ai_network(format!("bad SSE json: {e}")))?;
            let content = v
                .pointer("/choices/0/delta/content")
                .and_then(|c| c.as_str())
                .unwrap_or("");
            Ok((!content.is_empty()).then(|| content.to_string()))
        })
        .await
    }
}

#[async_trait]
impl AiProvider for OpenAiProvider {
    fn label(&self) -> String {
        self.label.clone()
    }

    async fn stream_complete(
        &self,
        req: CompletionRequest,
        cancel: Option<&CancelToken>,
        on_delta: OnDelta,
    ) -> Result<String, AppError> {
        self.send(req, cancel, on_delta).await
    }
}

// =====================
// Anthropic Messages API
// =====================

pub struct AnthropicProvider {
    url: String,
    key: String,
    model: String,
    client: reqwest::Client,
    idle_timeout: Duration,
    label: String,
}

impl AnthropicProvider {
    pub fn new(
        base: String,
        key: String,
        model: String,
        net: &SharedNetConfig,
        timeout_secs: u32,
        label: String,
    ) -> Self {
        // base = API 根；已带 /v1 的直接接 /messages。
        let url = if base.trim_end_matches('/').ends_with("/v1") {
            format!("{}/messages", base.trim_end_matches('/'))
        } else {
            format!("{}/v1/messages", base.trim_end_matches('/'))
        };
        Self {
            url,
            key,
            model,
            client: build_client(net, None),
            idle_timeout: Duration::from_secs(timeout_secs.max(1) as u64),
            label,
        }
    }
}

#[async_trait]
impl AiProvider for AnthropicProvider {
    fn label(&self) -> String {
        self.label.clone()
    }

    async fn stream_complete(
        &self,
        req: CompletionRequest,
        cancel: Option<&CancelToken>,
        on_delta: OnDelta,
    ) -> Result<String, AppError> {
        let body = json!({
            "model": self.model,
            "max_tokens": req.max_tokens,
            "system": req.system,
            "messages": [{"role": "user", "content": req.user}],
            "stream": true,
            "temperature": 0.4,
        });
        // 取消在请求建立阶段同样生效。
        let request_fut = self
            .client
            .post(&self.url)
            .header("content-type", "application/json")
            .header("x-api-key", &self.key)
            .header("anthropic-version", "2023-06-01")
            .body(body.to_string())
            .send();
        let response = match cancel {
            Some(token) => tokio::select! {
                biased;
                _ = token.cancelled() => return Err(AppError::OperationCancelled),
                resp = request_fut => resp.map_err(map_transport_error)?,
            },
            None => request_fut.await.map_err(map_transport_error)?,
        };
        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(map_http_error(status.as_u16(), &text));
        }

        stream_sse(response, self.idle_timeout, cancel, on_delta, |data| {
            let v: serde_json::Value = serde_json::from_str(data)
                .map_err(|e| AppError::ai_network(format!("bad SSE json: {e}")))?;
            match v.get("type").and_then(|t| t.as_str()) {
                Some("content_block_delta") => {
                    let text = v
                        .pointer("/delta/text")
                        .and_then(|t| t.as_str())
                        .unwrap_or("");
                    Ok((!text.is_empty()).then(|| text.to_string()))
                }
                Some("error") => {
                    let msg = v
                        .pointer("/error/message")
                        .and_then(|m| m.as_str())
                        .unwrap_or("stream error")
                        .to_string();
                    Err(AppError::AiProvider {
                        status: 200,
                        message: msg,
                    })
                }
                // message_stop / ping / 其他事件：不产出文本。
                _ => Ok(None),
            }
        })
        .await
    }
}

// =====================
// 共享 SSE 流循环
// =====================

/// 通用流循环：逐块喂 SSE 解析器，`extract` 从事件负载提取增量文本；
/// `Err` 立即中止，`Ok(None)` 跳过，`Ok(Some(chunk))` 推 `on_delta` 并累计。
async fn stream_sse(
    response: reqwest::Response,
    idle_timeout: Duration,
    cancel: Option<&CancelToken>,
    on_delta: OnDelta,
    mut extract: impl FnMut(&str) -> Result<Option<String>, AppError>,
) -> Result<String, AppError> {
    let mut parser = SseParser::new();
    let mut full = String::new();
    let mut stream = response.bytes_stream();

    loop {
        let chunk = {
            // 取消与空闲超时都在「等待下一块」处生效：取消即丢弃连接；
            // 空闲超时覆盖断网 / 服务端挂起（总时长由用户取消兜底）。
            tokio::select! {
                biased;
                _ = async {
                    match cancel {
                        Some(t) => t.cancelled().await,
                        None => std::future::pending::<()>().await,
                    }
                } => return Err(AppError::OperationCancelled),
                chunk = tokio::time::timeout(idle_timeout, stream.next()) => {
                    match chunk {
                        Err(_elapsed) => {
                            return Err(AppError::ai_network(format!(
                                "idle timeout after {}s (stream stalled)",
                                idle_timeout.as_secs()
                            )));
                        }
                        Ok(Some(Err(e))) => return Err(map_transport_error(e)),
                        Ok(Some(Ok(bytes))) => bytes,
                        Ok(None) => break, // 流正常结束
                    }
                }
            }
        };

        for event in parser.feed(&chunk) {
            if event.data.is_empty() {
                continue;
            }
            if let Some(text) = extract(&event.data)? {
                on_delta(&text);
                full.push_str(&text);
            }
        }
    }
    for event in parser.finish() {
        if let Some(text) = extract(&event.data)? {
            on_delta(&text);
            full.push_str(&text);
        }
    }
    Ok(full)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(url: String, auth: AuthStyle, key: &str) -> OpenAiProvider {
        OpenAiProvider::new(
            url,
            key.to_string(),
            auth,
            "test-model".into(),
            &SharedNetConfig::new(),
            10,
            "test".into(),
        )
    }

    // ---------- 错误映射 ----------

    #[test]
    fn http_errors_map_to_distinct_codes() {
        let auth = map_http_error(401, r#"{"error":{"message":"bad key"}}"#);
        assert!(matches!(auth, AppError::AiAuth { status: 401, .. }));
        let limit = map_http_error(429, r#"{"error":{"message":"quota"}}"#);
        assert!(matches!(limit, AppError::AiRateLimit { status: 429, .. }));
        let other = map_http_error(500, "boom");
        assert!(matches!(other, AppError::AiProvider { status: 500, .. }));
        // 非 JSON 体 → 截断为短字符串。
        let other2 = map_http_error(502, "bad gateway");
        assert!(matches!(
            other2,
            AppError::AiProvider {
                message,
                ..
            } if message == "bad gateway"
        ));
    }

    #[test]
    fn completions_url_normalizes_trailing_slash() {
        assert_eq!(
            OpenAiProvider::completions_url("http://localhost:11434/v1/"),
            "http://localhost:11434/v1/chat/completions"
        );
    }

    // ---------- wiremock 端到端（SSE 流式） ----------

    #[tokio::test(flavor = "multi_thread")]
    async fn openai_stream_emits_deltas_and_returns_full_text() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/chat/completions"))
            .respond_with(move |_req: &wiremock::Request| {
                wiremock::ResponseTemplate::new(200).set_body_string(
                    "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\n\
                     data: {\"choices\":[{\"delta\":{\"content\":\" world\"}}]}\n\n\
                     data: [DONE]\n\n",
                )
            })
            .mount(&server)
            .await;

        let p = provider(
            server.uri() + "/chat/completions",
            AuthStyle::Bearer,
            "sk-x",
        );
        let seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let sink = seen.clone();
        let text = p
            .stream_complete(
                CompletionRequest {
                    system: "s".into(),
                    user: "u".into(),
                    max_tokens: 64,
                },
                None,
                Arc::new(move |d: &str| sink.lock().unwrap().push(d.to_string())),
            )
            .await
            .unwrap();
        assert_eq!(text, "Hello world");
        assert_eq!(*seen.lock().unwrap(), vec!["Hello", " world"]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn openai_stream_sends_bearer_and_model() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::header(
                "authorization",
                "Bearer sk-live",
            ))
            .and(wiremock::matchers::body_partial_json(
                serde_json::json!({"model": "test-model", "stream": true}),
            ))
            .respond_with(|_: &wiremock::Request| {
                wiremock::ResponseTemplate::new(200)
                    .set_body_string("data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\n")
            })
            .mount(&server)
            .await;
        let p = provider(
            server.uri() + "/chat/completions",
            AuthStyle::Bearer,
            "sk-live",
        );
        let text = p
            .stream_complete(
                CompletionRequest {
                    system: "s".into(),
                    user: "u".into(),
                    max_tokens: 16,
                },
                None,
                Arc::new(|_| {}),
            )
            .await
            .unwrap();
        assert_eq!(text, "ok");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn openai_401_maps_to_ai_auth() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(|_: &wiremock::Request| {
                wiremock::ResponseTemplate::new(401)
                    .set_body_string(r#"{"error":{"message":"invalid api key"}}"#)
            })
            .mount(&server)
            .await;
        let p = provider(
            server.uri() + "/chat/completions",
            AuthStyle::Bearer,
            "wrong",
        );
        let err = p
            .stream_complete(
                CompletionRequest {
                    system: "s".into(),
                    user: "u".into(),
                    max_tokens: 16,
                },
                None,
                Arc::new(|_| {}),
            )
            .await
            .unwrap_err();
        assert!(
            matches!(err, AppError::AiAuth { status: 401, .. }),
            "{err:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn unreachable_endpoint_maps_to_ai_network() {
        // 连接一个必然拒绝的端口。
        let p = provider(
            "http://127.0.0.1:9/v1/chat/completions".into(),
            AuthStyle::None,
            "",
        );
        let err = p
            .stream_complete(
                CompletionRequest {
                    system: "s".into(),
                    user: "u".into(),
                    max_tokens: 16,
                },
                None,
                Arc::new(|_| {}),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::AiNetwork { .. }), "{err:?}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn idle_timeout_maps_to_ai_network() {
        // wiremock 的 set_delay 只延迟响应开始，无法模拟「流中途停帧」；
        // 手写 TCP server：发出一个事件后挂住不发也不关 —— 空闲超时必须触发。
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            // std::net 创建的是阻塞 socket；tokio 1.47+ 在 macOS (kqueue)
            // 上注册阻塞 fd 会 panic（tokio-rs/tokio#7172），必须先转非阻塞。
            listener.set_nonblocking(true).unwrap();
            let listener = tokio::net::TcpListener::from_std(listener).unwrap();
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = stream;
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut buf = [0u8; 8192];
            // 读掉请求头（直到 \r\n\r\n）。
            let _ = tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    let n = stream.read(&mut buf).await.unwrap_or(0);
                    if n == 0 || buf[..n].windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
            })
            .await;
            let _ = stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n",
                )
                .await;
            let _ = stream
                .write_all(b"data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}\n\n")
                .await;
            // 挂住连接不发后续（连接保持打开，流不结束）。
            tokio::time::sleep(Duration::from_secs(30)).await;
        });
        // timeout_secs = 1 → 空闲超时 1s。
        let p = OpenAiProvider::new(
            format!("http://{addr}/v1/chat/completions"),
            String::new(),
            AuthStyle::None,
            "test-model".into(),
            &SharedNetConfig::new(),
            1,
            "test".into(),
        );
        let err = p
            .stream_complete(
                CompletionRequest {
                    system: "s".into(),
                    user: "u".into(),
                    max_tokens: 16,
                },
                None,
                Arc::new(|_| {}),
            )
            .await
            .unwrap_err();
        assert!(
            matches!(err, AppError::AiNetwork { ref message } if message.contains("idle timeout")),
            "{err:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cancellation_aborts_the_stream() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(|_: &wiremock::Request| {
                wiremock::ResponseTemplate::new(200)
                    .set_body_string("data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}\n\n")
                    .set_delay(Duration::from_secs(10))
            })
            .mount(&server)
            .await;
        let p = provider(server.uri() + "/chat/completions", AuthStyle::None, "");
        let token = CancelToken::new();
        let t2 = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            t2.cancel();
        });
        let started = std::time::Instant::now();
        let err = p
            .stream_complete(
                CompletionRequest {
                    system: "s".into(),
                    user: "u".into(),
                    max_tokens: 16,
                },
                Some(&token),
                Arc::new(|_| {}),
            )
            .await
            .unwrap_err();
        assert!(
            matches!(err, AppError::OperationCancelled),
            "expect cancel, got {err:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn anthropic_stream_extracts_text_deltas() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::header("x-api-key", "sk-a"))
            .and(wiremock::matchers::header(
                "anthropic-version",
                "2023-06-01",
            ))
            .respond_with(|_: &wiremock::Request| {
                wiremock::ResponseTemplate::new(200).set_body_string(
                    "event: content_block_delta\n\
                     data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Hi\"}}\n\n\
                     event: ping\n\
                     data: {\"type\":\"ping\"}\n\n\
                     event: content_block_delta\n\
                     data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"!\"}}\n\n\
                     event: message_stop\n\
                     data: {\"type\":\"message_stop\"}\n\n",
                )
            })
            .mount(&server)
            .await;
        let p = AnthropicProvider::new(
            server.uri(),
            "sk-a".into(),
            "claude-test".into(),
            &SharedNetConfig::new(),
            10,
            "anthropic-test".into(),
        );
        let text = p
            .stream_complete(
                CompletionRequest {
                    system: "s".into(),
                    user: "u".into(),
                    max_tokens: 32,
                },
                None,
                Arc::new(|_| {}),
            )
            .await
            .unwrap();
        assert_eq!(text, "Hi!");
        assert_eq!(p.label(), "anthropic-test");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn anthropic_midstream_error_is_reported() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(|_: &wiremock::Request| {
                wiremock::ResponseTemplate::new(200).set_body_string(
                    "event: error\n\
                     data: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}\n\n",
                )
            })
            .mount(&server)
            .await;
        let p = AnthropicProvider::new(
            server.uri(),
            "sk-a".into(),
            "claude-test".into(),
            &SharedNetConfig::new(),
            10,
            "anthropic-test".into(),
        );
        let err = p
            .stream_complete(
                CompletionRequest {
                    system: "s".into(),
                    user: "u".into(),
                    max_tokens: 32,
                },
                None,
                Arc::new(|_| {}),
            )
            .await
            .unwrap_err();
        assert!(
            matches!(err, AppError::AiProvider { ref message, .. } if message.contains("Overloaded")),
            "{err:?}"
        );
    }
}
