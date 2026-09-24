//! CredentialBroker（P7，ADR-004）。
//!
//! 职责三分（PLAN P7）：
//! - **Keychain 读写**：机密只存 OS 凭据库（keyring crate），磁盘索引不含明文；
//! - **UI 请求**：凭据/口令短语/host key 事件 → 前端对话框 → `credential_respond` 回传；
//! - **会话缓存**：内存级，get 命中免弹窗，erase 时同步清除。
//!
//! 回连通道（§4.9 边界矩阵）：Windows Named Pipe（显式 DACL 仅当前用户 SID）/
//! Linux UDS（`$XDG_RUNTIME_DIR`，0600 + SO_PEERCRED）/ macOS UDS（`$TMPDIR`，
//! 0600 + getpeereid）；连接需握手 token（env 注入，首消息校验）；
//! 启动时清理僵尸 socket（ECONNREFUSED → unlink 重建）。
//!
//! 取消流：helper 收到 `cancelled` → stderr 标记行 → 引擎映射
//! `AppError::CredentialCancelled`（见设计文档 §5）。

use crate::core::error::AppError;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// helper 在用户取消时输出的 stderr 标记；经 git 的 stderr 一路透传，
/// 引擎据此把网络命令失败映射为 `CredentialCancelled`（无跨进程状态）。
pub const CREDENTIAL_CANCELLED_MARKER: &str = "ibexgit: credential-cancelled";

/// 凭据弹窗等待上限：超时按取消处理，防止前端异常导致 git 永久挂起。
const UI_WAIT: Duration = Duration::from_secs(300);

// =====================
// 通道协议（JSON Lines）
// =====================

#[derive(Debug, Serialize, Deserialize)]
pub struct Hello {
    pub v: u32,
    pub token: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HelloAck {
    pub v: u32,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub err: Option<String>,
}

/// helper → broker 的单条请求（一次连接一条）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CredRequest {
    pub id: String,
    /// `get` | `store` | `erase` | `askpass`
    pub op: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// `store` 时的机密
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
    /// `askpass` 时的提示文本
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// helper 的父进程（即 git 进程）pid，仅日志追踪。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ppid: Option<u32>,
}

/// broker → helper 的应答。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CredResponse {
    pub id: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancelled: Option<bool>,
}

impl CredResponse {
    fn credential(id: &str, cred: &StoredCred) -> Self {
        Self {
            id: id.to_string(),
            ok: true,
            username: Some(cred.username.clone()),
            secret: Some(cred.secret.clone()),
            answer: None,
            cancelled: None,
        }
    }

    fn answer(id: &str, answer: &str) -> Self {
        Self {
            id: id.to_string(),
            ok: true,
            username: None,
            secret: None,
            answer: Some(answer.to_string()),
            cancelled: None,
        }
    }

    fn cancelled(id: &str) -> Self {
        Self {
            id: id.to_string(),
            ok: false,
            username: None,
            secret: None,
            answer: None,
            cancelled: Some(true),
        }
    }

    fn ok(id: &str) -> Self {
        Self {
            id: id.to_string(),
            ok: true,
            username: None,
            secret: None,
            answer: None,
            cancelled: None,
        }
    }
}

// =====================
// UI 侧类型（specta）
// =====================

/// `credential://request` 事件负载：前端据此渲染三动作对话框
/// （Submit / Cancel / Remember）。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, tauri_specta::Event)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CredentialPrompt {
    /// HTTPS 凭据请求（credential helper get 未命中）。
    Https {
        request_id: String,
        protocol: String,
        host: String,
        path: Option<String>,
        username: Option<String>,
    },
    /// askpass 提示（口令短语 / 用户名 / 密码兜底）。
    Askpass {
        request_id: String,
        prompt: String,
        /// 提示是索取用户名（普通输入框）还是机密（密码框）。
        is_secret: bool,
    },
    /// SSH host key 首次确认（指纹展示 + 信任记忆）。
    HostKey {
        request_id: String,
        prompt: String,
        fingerprint: Option<String>,
        host: Option<String>,
    },
}

/// 前端 `credential_respond` 的应答（PLAN P7 三动作）。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum CredentialReply {
    /// HTTPS/askpass 提交；`remember` = 写入 keychain（批准后生效，见 §6）。
    Submit {
        username: Option<String>,
        secret: Option<String>,
        remember: bool,
    },
    /// host key：信任（应答 yes；remember = 存入信任库）。
    TrustHostKey { remember: bool },
    /// 用户取消 → CredentialCancelled。
    Cancel,
}

/// 凭据管理页条目（凭据索引，不含机密）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, specta::Type)]
pub struct CredentialEntry {
    pub key: String,
    pub host: String,
    pub username: String,
    /// Unix 秒（i32 到 2038 年足够，避免 specta BigInt 限制）。
    #[serde(with = "i64_as_string")]
    #[specta(type = String)]
    pub updated_at: i64,
}

mod i64_as_string {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(v: &i64, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&v.to_string())
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
        String::deserialize(d)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// 已信任的 SSH host key 条目。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, specta::Type)]
pub struct KnownHost {
    pub host: String,
    pub fingerprint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredCred {
    pub username: String,
    pub secret: String,
}

// =====================
// 存储层：keychain + 磁盘索引/信任库
// =====================

/// 机密存储抽象（keychain 生产实现 + 测试内存实现）。
pub trait CredentialStore: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<StoredCred>, String>;
    fn set(&self, key: &str, cred: &StoredCred) -> Result<(), String>;
    fn delete(&self, key: &str) -> Result<(), String>;
}

/// OS keychain（Windows 凭据管理器 / macOS Keychain / Linux Secret Service）。
/// password 字段存 JSON `{"username":…,"secret":…}`。
pub struct KeyringStore;

impl KeyringStore {
    fn entry(key: &str) -> Result<keyring::Entry, String> {
        keyring::Entry::new("IbexGit", key).map_err(|e| e.to_string())
    }
}

impl CredentialStore for KeyringStore {
    fn get(&self, key: &str) -> Result<Option<StoredCred>, String> {
        let entry = Self::entry(key)?;
        match entry.get_password() {
            Ok(json) => Ok(serde_json::from_str::<StoredCred>(&json).ok()),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    fn set(&self, key: &str, cred: &StoredCred) -> Result<(), String> {
        let entry = Self::entry(key)?;
        let json = serde_json::to_string(cred).map_err(|e| e.to_string())?;
        entry.set_password(&json).map_err(|e| e.to_string())
    }

    fn delete(&self, key: &str) -> Result<(), String> {
        let entry = Self::entry(key)?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// 进程内存储（单测 / 无 keychain 环境的通道集成测试）。
#[derive(Default)]
pub struct MemoryStore {
    map: Mutex<HashMap<String, StoredCred>>,
}

impl CredentialStore for MemoryStore {
    fn get(&self, key: &str) -> Result<Option<StoredCred>, String> {
        Ok(self.map.lock().unwrap().get(key).cloned())
    }
    fn set(&self, key: &str, cred: &StoredCred) -> Result<(), String> {
        self.map
            .lock()
            .unwrap()
            .insert(key.to_string(), cred.clone());
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<(), String> {
        self.map.lock().unwrap().remove(key);
        Ok(())
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct CredIndexFile {
    #[serde(default)]
    entries: Vec<CredentialEntry>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct KnownHostsFile {
    #[serde(default)]
    hosts: Vec<KnownHost>,
}

// =====================
// UI 桥
// =====================

/// 主程序侧的 UI 出口（lib.rs 用 Tauri 事件实现；测试用自动应答实现）。
/// 同步接口：事件发射本身不需要 await；等待应答由 broker 侧完成。
pub trait UiBridge: Send + Sync {
    fn show(&self, prompt: CredentialPrompt);
}

// =====================
// 纯函数：git credential 文本协议 / askpass 分类
// =====================

/// 解析 git credential 请求文本（stdin）：`key=value` 行，空行结束。
pub fn parse_credential_input(input: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in input.lines() {
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once('=') {
            map.insert(k.to_string(), v.to_string());
        }
    }
    map
}

/// 把 `get` 应答格式化为 git credential 输出（`username=`/`password=` + 空行）。
pub fn format_credential_output(username: &str, secret: &str) -> String {
    format!("username={username}\npassword={secret}\n\n")
}

/// 凭据的稳定 key：`<protocol>://<host><path>`（缺省回落 host）。
pub fn credential_key(protocol: Option<&str>, host: &str, path: Option<&str>) -> String {
    match (protocol, path) {
        (Some(p), Some(path)) if !p.is_empty() => {
            let path = if path.starts_with('/') {
                path.to_string()
            } else {
                format!("/{path}")
            };
            format!("{p}://{host}{path}")
        }
        (Some(p), _) if !p.is_empty() => format!("{p}://{host}"),
        _ => host.to_string(),
    }
}

/// askpass 提示分类（设计文档 §7）：host key 新增 / 机密 / 通用文本。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AskpassKind {
    /// 新主机指纹确认（指纹已提取）。
    HostKeyNew {
        host: Option<String>,
        fingerprint: String,
    },
    /// key 变更警告：ssh 实际不会走到询问，这里防御性识别并拒绝。
    HostKeyChanged,
    /// 口令短语 / 密码（key_hint = 提示中提到的密钥路径，用于按 key 记忆）。
    Secret { key_hint: Option<String> },
    /// 用户名或其它文本。
    Generic,
}

fn sha256_fingerprint(text: &str) -> Option<String> {
    let re = Regex::new(r"SHA256:[A-Za-z0-9+/=]+").ok()?;
    re.find(text).map(|m| m.as_str().to_string())
}

fn quoted_host(text: &str) -> Option<String> {
    let re = Regex::new(r"'([^']+) \(").ok()?;
    re.captures(text).map(|c| c[1].to_string())
}

fn key_path_hint(text: &str) -> Option<String> {
    // "Enter passphrase for key '/c/Users/x/.ssh/id_ed25519': "
    let re = Regex::new(r"key '([^']+)'").ok()?;
    re.captures(text).map(|c| c[1].to_string())
}

pub fn classify_askpass(prompt: &str) -> AskpassKind {
    if prompt.contains("REMOTE HOST IDENTIFICATION HAS CHANGED")
        || prompt.contains("POSSIBLE DNS SPOOFING")
    {
        return AskpassKind::HostKeyChanged;
    }
    if let Some(fp) = sha256_fingerprint(prompt) {
        return AskpassKind::HostKeyNew {
            host: quoted_host(prompt),
            fingerprint: fp,
        };
    }
    let lower = prompt.to_lowercase();
    if lower.contains("passphrase")
        || lower.contains("password")
        || lower.contains("passcode")
        || lower.contains("token")
    {
        return AskpassKind::Secret {
            key_hint: key_path_hint(prompt),
        };
    }
    AskpassKind::Generic
}

// =====================
// Broker
// =====================

pub struct CredentialBroker {
    data_dir: PathBuf,
    token: String,
    addr: String,
    store: Box<dyn CredentialStore>,
    bridge: Arc<dyn UiBridge>,
    /// 会话缓存（进程生命周期）。
    session: Mutex<HashMap<String, StoredCred>>,
    /// 批准意图：get 时勾选"记住" → store 动作（认证成功）真正落 keychain。
    approvals: Mutex<HashMap<String, bool>>,
    /// 等待 UI 应答的请求（request_id → sender；respond 取出即一次性）。
    pending: Mutex<HashMap<String, std::sync::mpsc::Sender<CredentialReply>>>,
    next_id: AtomicU64,
    /// UDS 泄漏守卫：Drop 时 unlink socket 文件。
    #[allow(dead_code)]
    cleanup: Mutex<Option<PathBuf>>,
}

impl CredentialBroker {
    /// 启动 broker + 回连通道 server。`helper_path` 供 `spawn_injection()` 使用。
    pub fn start(data_dir: PathBuf, bridge: Arc<dyn UiBridge>) -> Result<Arc<Self>, AppError> {
        std::fs::create_dir_all(credentials_dir(&data_dir))
            .map_err(|e| AppError::io_with_detail(e.to_string(), "create credentials dir"))?;

        let token = random_token();
        let addr = channel_address();
        tracing::info!(addr = %addr, "credential channel");
        let broker = Arc::new(Self {
            data_dir,
            token,
            addr: addr.clone(),
            store: Box::new(KeyringStore),
            bridge,
            session: Mutex::new(HashMap::new()),
            approvals: Mutex::new(HashMap::new()),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            cleanup: Mutex::new(None),
        });

        let channel_addr = if cfg!(windows) {
            ChannelAddr::NamedPipe(addr.clone())
        } else {
            ChannelAddr::UnixSocket(PathBuf::from(&addr))
        };
        start_server(broker.clone(), channel_addr, broker.token.clone())
            .map_err(|e| AppError::io_with_detail(e.to_string(), "start credential channel"))?;
        Ok(broker)
    }

    /// 测试专用构造：注入存储实现（keychain 在 CI 中不可用）。
    pub fn with_store(
        data_dir: PathBuf,
        bridge: Arc<dyn UiBridge>,
        store: Box<dyn CredentialStore>,
        addr: String,
    ) -> Arc<Self> {
        Arc::new(Self {
            data_dir,
            token: random_token(),
            addr,
            store,
            bridge,
            session: Mutex::new(HashMap::new()),
            approvals: Mutex::new(HashMap::new()),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            cleanup: Mutex::new(None),
        })
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    pub fn addr(&self) -> &str {
        &self.addr
    }

    /// Runner 注入配置（ADR-014：系统 helper 优先）。
    ///
    /// **追加链尾，不重置**：`-c credential.helper=<ours>` 多值 append 在
    /// system/global 配置的 helper（GCM/osxkeychain…）之后——缓存命中时静默
    /// 复用系统已验证的认证，未命中时弹其原生授权窗口；全链未命中才轮到
    /// IbexGit 的对话框（兜底覆盖无 helper 环境）。过期凭据由 git 的
    /// reject/erase 循环自动清理。
    ///
    /// 静态标记 `ibexgit-credential`：git 会在命令末尾追加操作名，helper
    /// 据此分流（credential 模式）；helper 入口同步接受该标记。
    ///
    /// `!` 前缀（shell 形式）必不可少：git 对不以 `/` 或 `.` 开头的值会
    /// 当作 `git credential-<name>` 子命令去解析，Windows 路径加引号后
    /// 首字符是 `"`，同样落入子命令分支（实测 helper 根本不被执行，
    /// 见 gitcredentials(7)）。`!` 强制整串走 shell，引号得以保留，
    /// 含空格路径也安全。
    ///
    /// askpass 桥（GIT/SSH_ASKPASS）保持注入：SSH 口令短语、host key 确认、
    /// 以及 helper 全部未命中时 git 的最后追问（GIT_TERMINAL_PROMPT=0 下转
    /// askpass）都走应用内 UI，取消流语义完整。
    pub fn spawn_injection(&self, helper_path: &Path) -> SpawnInjection {
        let helper = helper_path.to_string_lossy().replace('\\', "/");
        SpawnInjection {
            args: vec![
                "-c".to_string(),
                // git 经 shell 执行 helper 命令：双引号包裹路径（正斜杠），
                // action（get/store/erase）由 git 追加。无空值重置：不屏蔽
                // 用户全局 helper（ADR-014）。
                format!("credential.helper=!\"{helper}\" ibexgit-credential",),
            ],
            env: vec![
                (
                    "GIT_ASKPASS".to_string(),
                    helper_path.to_string_lossy().into_owned(),
                ),
                (
                    "SSH_ASKPASS".to_string(),
                    helper_path.to_string_lossy().into_owned(),
                ),
                ("SSH_ASKPASS_REQUIRE".to_string(), "force".to_string()),
                ("IBEXGIT_CRED_TOKEN".to_string(), self.token.clone()),
                ("IBEXGIT_CRED_ADDR".to_string(), self.addr.clone()),
            ],
        }
    }

    fn next_request_id(&self) -> String {
        let n = self.next_id.fetch_add(1, Ordering::Relaxed);
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        format!("r{ts}-{n}")
    }

    /// 通道线程调用的同步入口。
    pub fn handle(&self, req: CredRequest) -> CredResponse {
        match req.op.as_str() {
            "get" => self.handle_get(&req),
            "store" => self.handle_store(&req),
            "erase" => self.handle_erase(&req),
            "askpass" => self.handle_askpass(&req),
            other => {
                tracing::warn!(op = other, "unknown credential op");
                CredResponse::cancelled(&req.id)
            }
        }
    }

    fn handle_get(&self, req: &CredRequest) -> CredResponse {
        let Some(host) = req.host.as_deref().filter(|h| !h.is_empty()) else {
            return CredResponse::cancelled(&req.id);
        };
        let key = credential_key(req.protocol.as_deref(), host, req.path.as_deref());

        // 1) 会话缓存
        if let Some(cred) = self.session.lock().unwrap().get(&key) {
            tracing::info!(host, "credential: session cache hit");
            return CredResponse::credential(&req.id, cred);
        }
        // 2) keychain
        match self.store.get(&key) {
            Ok(Some(cred)) => {
                tracing::info!(host, "credential: keychain hit");
                self.session
                    .lock()
                    .unwrap()
                    .insert(key.clone(), cred.clone());
                return CredResponse::credential(&req.id, &cred);
            }
            Ok(None) => {}
            Err(e) => tracing::warn!(host, error = %e, "keychain read failed"),
        }

        // 3) UI 请求
        let protocol = req.protocol.clone().unwrap_or_else(|| "https".to_string());
        let (tx, rx) = std::sync::mpsc::channel::<CredentialReply>();
        let request_id = self.next_request_id();
        self.pending.lock().unwrap().insert(request_id.clone(), tx);
        self.bridge.show(CredentialPrompt::Https {
            request_id: request_id.clone(),
            protocol,
            host: host.to_string(),
            path: req.path.clone(),
            username: req.username.clone(),
        });
        match rx.recv_timeout(UI_WAIT) {
            Ok(CredentialReply::Submit {
                username,
                secret,
                remember,
            }) => {
                let Some(secret) = secret.filter(|s| !s.is_empty()) else {
                    return CredResponse::cancelled(&req.id);
                };
                let username = username.unwrap_or_default();
                let cred = StoredCred {
                    username: username.clone(),
                    secret: secret.clone(),
                };
                self.session
                    .lock()
                    .unwrap()
                    .insert(key.clone(), cred.clone());
                if remember {
                    self.approvals.lock().unwrap().insert(key, true);
                }
                CredResponse::credential(&req.id, &cred)
            }
            Ok(CredentialReply::Cancel) | Err(_) => {
                self.pending.lock().unwrap().remove(&request_id);
                CredResponse::cancelled(&req.id)
            }
            Ok(_) => CredResponse::cancelled(&req.id),
        }
    }

    fn handle_store(&self, req: &CredRequest) -> CredResponse {
        let (Some(secret), Some(host)) = (req.secret.as_deref(), req.host.as_deref()) else {
            return CredResponse::ok(&req.id);
        };
        if secret.is_empty() {
            return CredResponse::ok(&req.id);
        }
        let key = credential_key(req.protocol.as_deref(), host, req.path.as_deref());
        let cred = StoredCred {
            username: req.username.clone().unwrap_or_default(),
            secret: secret.to_string(),
        };
        // 批准的凭据永远进会话缓存（认证已成功）。
        self.session
            .lock()
            .unwrap()
            .insert(key.clone(), cred.clone());
        // 只有 get 时勾选了"记住"才落 keychain（密钥错误不产生脏存储）。
        let remember = *self.approvals.lock().unwrap().get(&key).unwrap_or(&false);
        if remember {
            match self.store.set(&key, &cred) {
                Ok(()) => {
                    tracing::info!(host, "credential stored to keychain");
                    if let Err(e) = self.index_upsert(&key, host, &cred.username) {
                        tracing::warn!(error = %e, "credential index write failed");
                    }
                }
                Err(e) => tracing::warn!(host, error = %e, "keychain write failed"),
            }
        }
        CredResponse::ok(&req.id)
    }

    fn handle_erase(&self, req: &CredRequest) -> CredResponse {
        let Some(host) = req.host.as_deref() else {
            return CredResponse::ok(&req.id);
        };
        let key = credential_key(req.protocol.as_deref(), host, req.path.as_deref());
        self.session.lock().unwrap().remove(&key);
        self.approvals.lock().unwrap().remove(&key);
        if let Err(e) = self.store.delete(&key) {
            tracing::warn!(host, error = %e, "keychain delete failed");
        } else {
            tracing::info!(host, "credential erased (rejected by remote)");
        }
        if let Err(e) = self.index_remove(&key) {
            tracing::warn!(error = %e, "credential index write failed");
        }
        CredResponse::ok(&req.id)
    }

    fn handle_askpass(&self, req: &CredRequest) -> CredResponse {
        let Some(prompt) = req.prompt.as_deref() else {
            return CredResponse::cancelled(&req.id);
        };
        match classify_askpass(prompt) {
            // ssh 本身不会询问 key 变更（直接失败）；防御性拒绝。
            AskpassKind::HostKeyChanged => CredResponse::cancelled(&req.id),
            AskpassKind::HostKeyNew { host, fingerprint } => {
                self.askpass_host_key(req, prompt, host, fingerprint)
            }
            AskpassKind::Secret { key_hint } => {
                // 口令短语按密钥路径记忆（可选），命中则免弹窗。
                if let Some(hint) = key_hint.as_deref() {
                    let key = format!("ssh:{hint}");
                    if let Ok(Some(cred)) = self.store.get(&key) {
                        tracing::info!("askpass: remembered passphrase hit");
                        return CredResponse::answer(&req.id, &cred.secret);
                    }
                }
                let (tx, rx) = std::sync::mpsc::channel::<CredentialReply>();
                let request_id = self.next_request_id();
                self.pending.lock().unwrap().insert(request_id.clone(), tx);
                self.bridge.show(CredentialPrompt::Askpass {
                    request_id: request_id.clone(),
                    prompt: prompt.to_string(),
                    is_secret: true,
                });
                match rx.recv_timeout(UI_WAIT) {
                    Ok(CredentialReply::Submit {
                        secret, remember, ..
                    }) => {
                        let Some(secret) = secret.filter(|s| !s.is_empty()) else {
                            return CredResponse::cancelled(&req.id);
                        };
                        if remember {
                            if let Some(hint) = key_hint.as_deref() {
                                let key = format!("ssh:{hint}");
                                let cred = StoredCred {
                                    username: String::new(),
                                    secret: secret.clone(),
                                };
                                if let Err(e) = self.store.set(&key, &cred) {
                                    tracing::warn!(error = %e, "passphrase store failed");
                                }
                            }
                        }
                        CredResponse::answer(&req.id, &secret)
                    }
                    _ => CredResponse::cancelled(&req.id),
                }
            }
            AskpassKind::Generic => {
                let is_secret = !prompt.to_lowercase().contains("username");
                let (tx, rx) = std::sync::mpsc::channel::<CredentialReply>();
                let request_id = self.next_request_id();
                self.pending.lock().unwrap().insert(request_id.clone(), tx);
                self.bridge.show(CredentialPrompt::Askpass {
                    request_id: request_id.clone(),
                    prompt: prompt.to_string(),
                    is_secret,
                });
                match rx.recv_timeout(UI_WAIT) {
                    Ok(CredentialReply::Submit {
                        username, secret, ..
                    }) => {
                        // "Username for '…'" 提示取 username 字段，其余取 secret。
                        let answer = if is_secret { secret } else { username };
                        match answer.filter(|s| !s.is_empty()) {
                            Some(text) => CredResponse::answer(&req.id, &text),
                            None => CredResponse::cancelled(&req.id),
                        }
                    }
                    _ => CredResponse::cancelled(&req.id),
                }
            }
        }
    }

    fn askpass_host_key(
        &self,
        req: &CredRequest,
        prompt: &str,
        host: Option<String>,
        fingerprint: String,
    ) -> CredResponse {
        // 信任记忆：指纹一致 → 自动应答 yes（ssh 会把 key 写入用户 known_hosts）。
        if let Some(host) = host.as_deref() {
            if self.is_trusted(host, &fingerprint) {
                tracing::info!(host, "host key: trusted fingerprint, auto-accept");
                return CredResponse::answer(&req.id, "yes");
            }
        }
        let (tx, rx) = std::sync::mpsc::channel::<CredentialReply>();
        let request_id = self.next_request_id();
        self.pending.lock().unwrap().insert(request_id.clone(), tx);
        self.bridge.show(CredentialPrompt::HostKey {
            request_id: request_id.clone(),
            prompt: prompt.to_string(),
            fingerprint: Some(fingerprint.clone()),
            host: host.clone(),
        });
        match rx.recv_timeout(UI_WAIT) {
            Ok(CredentialReply::TrustHostKey { remember }) => {
                if remember {
                    if let Some(host) = host.as_deref() {
                        if let Err(e) = self.trust_host(host, &fingerprint) {
                            tracing::warn!(host, error = %e, "trust store write failed");
                        }
                    }
                }
                CredResponse::answer(&req.id, "yes")
            }
            _ => CredResponse::cancelled(&req.id),
        }
    }

    /// 前端 `credential_respond`：取出 pending 应答（一次性）。
    pub fn respond(&self, request_id: &str, reply: CredentialReply) -> Result<(), AppError> {
        let tx = self.pending.lock().unwrap().remove(request_id);
        match tx {
            Some(tx) => {
                let _ = tx.send(reply);
                Ok(())
            }
            None => Err(AppError::parse(format!(
                "unknown credential request: {request_id}"
            ))),
        }
    }

    // ---- 磁盘索引 / 信任库 ----

    fn index_path(&self) -> PathBuf {
        credentials_dir(&self.data_dir).join("index.json")
    }

    fn trust_path(&self) -> PathBuf {
        credentials_dir(&self.data_dir).join("known-hosts.json")
    }

    fn index_upsert(&self, key: &str, host: &str, username: &str) -> Result<(), String> {
        let mut file: CredIndexFile = read_json_or_default(&self.index_path());
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        if let Some(e) = file.entries.iter_mut().find(|e| e.key == key) {
            e.username = username.to_string();
            e.updated_at = now;
        } else {
            file.entries.push(CredentialEntry {
                key: key.to_string(),
                host: host.to_string(),
                username: username.to_string(),
                updated_at: now,
            });
        }
        write_json(&self.index_path(), &file)
    }

    fn index_remove(&self, key: &str) -> Result<(), String> {
        let mut file: CredIndexFile = read_json_or_default(&self.index_path());
        file.entries.retain(|e| e.key != key);
        write_json(&self.index_path(), &file)
    }

    pub fn list_index(&self) -> Vec<CredentialEntry> {
        let file: CredIndexFile = read_json_or_default(&self.index_path());
        file.entries
    }

    /// 凭据管理页删除：keychain + 会话缓存 + 索引。
    pub fn delete_credential(&self, key: &str) -> Result<(), AppError> {
        self.session.lock().unwrap().remove(key);
        self.approvals.lock().unwrap().remove(key);
        self.store.delete(key).map_err(AppError::io)?;
        self.index_remove(key).map_err(AppError::io)?;
        Ok(())
    }

    /// 测试/诊断：直读存储层（keychain 或内存实现）。
    pub fn peek_stored(&self, key: &str) -> Option<StoredCred> {
        self.store.get(key).ok().flatten()
    }

    fn load_trust(&self) -> KnownHostsFile {
        read_json_or_default(&self.trust_path())
    }

    fn is_trusted(&self, host: &str, fingerprint: &str) -> bool {
        self.load_trust()
            .hosts
            .iter()
            .any(|h| h.host == host && h.fingerprint == fingerprint)
    }

    fn trust_host(&self, host: &str, fingerprint: &str) -> Result<(), String> {
        let mut file = self.load_trust();
        file.hosts.retain(|h| h.host != host);
        file.hosts.push(KnownHost {
            host: host.to_string(),
            fingerprint: fingerprint.to_string(),
        });
        write_json(&self.trust_path(), &file)
    }

    pub fn list_known_hosts(&self) -> Vec<KnownHost> {
        self.load_trust().hosts
    }

    pub fn remove_known_host(&self, host: &str) -> Result<(), AppError> {
        let mut file = self.load_trust();
        file.hosts.retain(|h| h.host != host);
        write_json(&self.trust_path(), &file).map_err(AppError::io)?;
        Ok(())
    }
}

fn credentials_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("credentials")
}

fn read_json_or_default<T: Default + serde::de::DeserializeOwned>(path: &Path) -> T {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
        Err(_) => T::default(),
    }
}

fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

/// CSPRNG 握手 token（32 字节 hex）。
pub fn random_token() -> String {
    let mut buf = [0u8; 32];
    getrandom::fill(&mut buf).expect("OS RNG unavailable");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

// =====================
// Runner 注入配置
// =====================

/// `GitProcessRunner` 的凭据注入配置（args + env，见 spawn_injection）。
#[derive(Debug, Clone, Default)]
pub struct SpawnInjection {
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

// =====================
// 回连通道 server
// =====================

#[derive(Debug, Clone)]
pub enum ChannelAddr {
    NamedPipe(String),
    UnixSocket(PathBuf),
}

/// 启动通道 server（阻塞线程 + 每连接一线程；请求频率极低，阻塞模型换取
/// ACL/对端校验代码的直白）。
/// 通道地址（用于 env 下发 + 日志）：Windows 命名管道带 pid 后缀
/// （多用户并存不撞名）；Unix 优先 `$XDG_RUNTIME_DIR` 固定名，回落
/// `$TMPDIR` 带 pid。与 start_server 内实现保持一致。
pub fn channel_address() -> String {
    #[cfg(windows)]
    {
        format!(r"\\.\pipe\ibexgit-credential-{}", std::process::id())
    }
    #[cfg(unix)]
    {
        unix::socket_path().display().to_string()
    }
    #[cfg(not(any(windows, unix)))]
    {
        String::new()
    }
}

/// 启动通道 server（阻塞线程 + 每连接一线程；请求频率极低，阻塞模型换取
/// ACL/对端校验代码的直白）。
fn start_server(
    broker: Arc<CredentialBroker>,
    addr: ChannelAddr,
    token: String,
) -> std::io::Result<()> {
    match addr {
        #[cfg(windows)]
        ChannelAddr::NamedPipe(name) => {
            let broker2 = broker.clone();
            std::thread::Builder::new()
                .name("credential-pipe".into())
                .spawn(move || windows_accept_loop(&name, token, broker2))?;
            Ok(())
        }
        #[cfg(unix)]
        ChannelAddr::UnixSocket(path) => unix::start(path, token, broker),
        // Windows 上无 UDS、Unix 上无命名管道：编译期存在但运行期不可达。
        #[cfg(windows)]
        ChannelAddr::UnixSocket(_) => Err(std::io::Error::other(
            "unix socket not supported on windows",
        )),
        #[cfg(unix)]
        ChannelAddr::NamedPipe(_) => Err(std::io::Error::other("named pipe not supported on unix")),
        #[cfg(not(any(windows, unix)))]
        _ => Err(std::io::Error::other(
            "credential channel unsupported on this platform",
        )),
    }
}

/// 测试/特殊部署入口：在指定地址启动通道 server。
pub fn start_channel_server(
    broker: Arc<CredentialBroker>,
    addr: ChannelAddr,
    token: String,
) -> std::io::Result<()> {
    start_server(broker, addr, token)
}

/// 单连接服务：握手 → 请求 → 应答（单连接单请求，helper 随即关闭）。
fn serve_connection<R: BufRead, W: Write>(
    mut reader: R,
    mut writer: W,
    token: &str,
    broker: &CredentialBroker,
) {
    let _ = writer.flush();
    // 握手（读超时由调用方 socket 设置）。
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
        return;
    }
    let hello: Result<Hello, _> = serde_json::from_str(line.trim());
    let hello = match hello {
        Ok(h) if h.token == token => h,
        _ => {
            tracing::warn!("credential channel: handshake rejected");
            let ack = serde_json::to_string(&HelloAck {
                v: 1,
                ok: false,
                err: Some("auth".into()),
            })
            .unwrap_or_default();
            let _ = writeln!(writer, "{ack}");
            return;
        }
    };
    let _ = hello;
    let ack = serde_json::to_string(&HelloAck {
        v: 1,
        ok: true,
        err: None,
    })
    .unwrap_or_default();
    if writeln!(writer, "{ack}").is_err() {
        return;
    }
    let _ = writer.flush();

    // 请求。
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
        return;
    }
    let Ok(req) = serde_json::from_str::<CredRequest>(line.trim()) else {
        tracing::warn!("credential channel: malformed request");
        return;
    };
    tracing::info!(op = %req.op, id = %req.id, "credential request");
    let resp = broker.handle(req);
    if let Ok(text) = serde_json::to_string(&resp) {
        let _ = writeln!(writer, "{text}");
        let _ = writer.flush();
    }
}

// =====================
// Windows Named Pipe
// =====================

#[cfg(windows)]
mod windows {
    use super::*;
    use std::ffi::c_void;
    use std::os::windows::io::{FromRawHandle, RawHandle};
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Pipes::{ConnectNamedPipe, DisconnectNamedPipe};

    /// 创建一个命名管道实例，显式 DACL 仅允许当前用户 token SID 读写。
    /// SD/ACL/token 缓冲与 CreateNamedPipeW 必须同帧：SD 内部含指向 ACL 与
    /// SID 的裸指针，一旦按值移出函数即悬空（ERROR_UNKNOWN_REVISION 的根源）。
    pub(crate) fn create_pipe_instance(name: &str, first: bool) -> std::io::Result<RawHandle> {
        use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE};
        use windows_sys::Win32::Security::{
            AddAccessAllowedAce, GetTokenInformation, InitializeAcl, InitializeSecurityDescriptor,
            SetSecurityDescriptorDacl, TokenUser, ACL, ACL_REVISION, SECURITY_ATTRIBUTES,
            SECURITY_DESCRIPTOR, TOKEN_QUERY, TOKEN_USER,
        };
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX,
        };
        use windows_sys::Win32::System::Pipes::{
            CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
            PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
        };
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

        unsafe {
            // 当前用户 token SID。
            let mut token = std::ptr::null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return Err(std::io::Error::last_os_error());
            }
            let mut len = 0u32;
            let _ = GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut len);
            if len == 0 {
                return Err(std::io::Error::last_os_error());
            }
            let mut info = vec![0u8; len as usize];
            if GetTokenInformation(
                token,
                TokenUser,
                info.as_mut_ptr() as *mut c_void,
                len,
                &mut len,
            ) == 0
            {
                return Err(std::io::Error::last_os_error());
            }
            let user = *(info.as_ptr() as *const TOKEN_USER);
            let sid = user.User.Sid;

            // 绝对格式 SD + 显式 DACL（仅当前用户）。
            let mut sd: SECURITY_DESCRIPTOR = std::mem::zeroed();
            // SECURITY_DESCRIPTOR_REVISION == 1（windows-sys 未导出常量）。
            if InitializeSecurityDescriptor(&mut sd as *mut _ as *mut c_void, 1) == 0 {
                return Err(std::io::Error::last_os_error());
            }
            // u32 数组保证 4 字节对齐（ACL 需 WORD 对齐）。
            let mut acl_buf = [0u32; 64];
            let acl = acl_buf.as_mut_ptr() as *mut ACL;
            if InitializeAcl(acl, 256, ACL_REVISION) == 0 {
                return Err(std::io::Error::last_os_error());
            }
            if AddAccessAllowedAce(acl, ACL_REVISION, GENERIC_READ | GENERIC_WRITE, sid) == 0 {
                return Err(std::io::Error::last_os_error());
            }
            if SetSecurityDescriptorDacl(&mut sd as *mut _ as *mut c_void, 1, acl, 0) == 0 {
                return Err(std::io::Error::last_os_error());
            }
            let sa = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: &sd as *const _ as *mut c_void,
                bInheritHandle: 0,
            };

            // 同帧调用：sa/sd/acl/info 全部存活。
            let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
            let sa_ptr = &sa as *const SECURITY_ATTRIBUTES;
            let open_mode = PIPE_ACCESS_DUPLEX
                | if first {
                    FILE_FLAG_FIRST_PIPE_INSTANCE
                } else {
                    0
                };
            let handle = CreateNamedPipeW(
                wide.as_ptr(),
                open_mode,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                4096,
                4096,
                0,
                sa_ptr,
            );
            if handle.is_null() || handle == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
                return Err(std::io::Error::last_os_error());
            }
            Ok(handle)
        }
    }

    /// 内核句柄上下文无关、非线程亲和，跨线程移交是安全的（同
    /// proctree::JobHandle 的论证）；所有权由移交方转移给连接线程。
    struct PipeHandle(RawHandle);
    // SAFETY: 见 doc comment。
    unsafe impl Send for PipeHandle {}

    /// 连接线程主体：handshake → 请求 → 应答 → 断开。
    fn serve_pipe_conn(conn: PipeHandle, token: String, broker: Arc<CredentialBroker>) {
        use windows_sys::Win32::Storage::FileSystem::FlushFileBuffers;
        let handle = conn.0;
        let file = std::mem::ManuallyDrop::new(unsafe { std::fs::File::from_raw_handle(handle) });
        let mut reader = BufReader::new(match file.try_clone() {
            Ok(w) => w,
            Err(_) => return,
        });
        serve_connection(&mut reader, &mut &*file, &token, &broker);
        unsafe {
            // 断开前冲刷：等客户端读走应答字节。否则 DisconnectNamedPipe 会
            // 丢弃未读数据，客户端读响应时报 ERROR_NO_DATA (233)。
            let _ = FlushFileBuffers(handle);
            DisconnectNamedPipe(handle);
            CloseHandle(handle);
        }
    }

    pub fn accept_loop(name: &str, token: String, broker: Arc<CredentialBroker>) {
        // FILE_FLAG_FIRST_PIPE_INSTANCE 只允许出现在首个实例上：首个实例仍在
        // 服务时创建第二个实例必须省略该标志（否则 ERROR_ACCESS_DENIED）。
        let mut first = true;
        loop {
            let handle = match create_pipe_instance(name, first) {
                Ok(h) => h,
                Err(e) => {
                    tracing::error!("CreateNamedPipeW failed: {e}");
                    std::thread::sleep(std::time::Duration::from_secs(1));
                    continue;
                }
            };
            first = false;
            // 阻塞等待客户端（PIPE_UNLIMITED_INSTANCES 允许排队）。
            let ok = unsafe { ConnectNamedPipe(handle, std::ptr::null_mut()) != 0 };
            if !ok {
                let err = std::io::Error::last_os_error();
                if err.raw_os_error() != Some(535) {
                    eprintln!("ACCEPT-DIAG: ConnectNamedPipe err={err}");
                    // ERROR_PIPE_CONNECTED: 客户端已抢先连接，仍可继续。
                    tracing::warn!("ConnectNamedPipe failed: {err}");
                    unsafe { CloseHandle(handle) };
                    continue;
                }
            }
            let broker = broker.clone();
            let token = token.clone();
            let conn = PipeHandle(handle);
            std::thread::Builder::new()
                .name("credential-conn".into())
                .spawn(move || serve_pipe_conn(conn, token, broker))
                .ok();
        }
    }
}

#[cfg(windows)]
fn windows_accept_loop(name: &str, token: String, broker: Arc<CredentialBroker>) {
    windows::accept_loop(name, token, broker)
}

// =====================
// Unix UDS
// =====================

#[cfg(unix)]
mod unix {
    use super::*;
    use std::io::ErrorKind;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::{UnixListener, UnixStream};

    /// 通道 socket 路径：优先 `$XDG_RUNTIME_DIR`（固定名），回落 `$TMPDIR`
    /// （追加 pid，见设计文档 §3 偏离说明）。
    pub fn socket_path() -> PathBuf {
        let xdg = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|p| p.is_dir());
        match xdg {
            Some(dir) => dir.join("ibexgit-credential.sock"),
            None => {
                let tmp = std::env::var_os("TMPDIR")
                    .map(PathBuf::from)
                    .filter(|p| p.is_dir())
                    .unwrap_or_else(|| PathBuf::from("/tmp"));
                tmp.join(format!("ibexgit-credential-{}.sock", std::process::id()))
            }
        }
    }

    /// 僵尸 socket 清理：路径已存在 → 尝试连接；成功 = 已有实例（报错）；
    /// ECONNREFUSED = 残留 → unlink 重建。
    fn cleanup_stale(path: &Path) -> std::io::Result<()> {
        if !path.exists() {
            return Ok(());
        }
        match UnixStream::connect(path) {
            Ok(_) => Err(std::io::Error::other(
                "another credential channel is already listening",
            )),
            Err(e) if e.kind() == ErrorKind::ConnectionRefused => {
                std::fs::remove_file(path)?;
                Ok(())
            }
            Err(_) => {
                // 其它错误（权限等）也按残留处理：同 uid 场景唯一成因是残留。
                std::fs::remove_file(path)?;
                Ok(())
            }
        }
    }

    pub fn start(
        path: PathBuf,
        token: String,
        broker: Arc<CredentialBroker>,
    ) -> std::io::Result<()> {
        cleanup_stale(&path)?;
        let listener = UnixListener::bind(&path)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        tracing::info!(path = %path.display(), "credential UDS listening");

        std::thread::Builder::new()
            .name("credential-uds".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    let Ok(stream) = stream else { continue };
                    let broker = broker.clone();
                    let token = token.clone();
                    std::thread::Builder::new()
                        .name("credential-conn".into())
                        .spawn(move || serve(stream, token, broker))
                        .ok();
                }
                let _ = std::fs::remove_file(&path);
            })?;
        Ok(())
    }

    /// 对端 uid 校验：Linux SO_PEERCRED / macOS getpeereid（§4.9 矩阵）。
    fn peer_uid(stream: &UnixStream) -> std::io::Result<u32> {
        use std::os::fd::AsRawFd;
        let fd = stream.as_raw_fd();
        #[cfg(target_os = "linux")]
        {
            let mut cred = libc::ucred {
                pid: 0,
                uid: u32::MAX,
                gid: 0,
            };
            let mut len = std::mem::size_of::<libc::ucred>() as u32;
            let rc = unsafe {
                libc::getsockopt(
                    fd,
                    libc::SOL_SOCKET,
                    libc::SO_PEERCRED,
                    &mut cred as *mut _ as *mut libc::c_void,
                    &mut len,
                )
            };
            if rc == 0 {
                Ok(cred.uid)
            } else {
                Err(std::io::Error::last_os_error())
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            let mut uid: u32 = u32::MAX;
            let mut gid: u32 = 0;
            let rc = unsafe { libc::getpeereid(fd, &mut uid, &mut gid) };
            if rc == 0 {
                Ok(uid)
            } else {
                Err(std::io::Error::last_os_error())
            }
        }
    }

    fn serve(stream: UnixStream, token: String, broker: Arc<CredentialBroker>) {
        // 对端身份：uid 不符立即断开（负向验收：同机其他用户进程不可连）。
        match peer_uid(&stream) {
            Ok(uid) if uid == nix_euid() => {}
            Ok(uid) => {
                tracing::warn!(uid, "credential channel: foreign uid rejected");
                return;
            }
            Err(e) => {
                tracing::warn!("credential channel: peer check failed: {e}");
                return;
            }
        }
        let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
        let mut reader = BufReader::new(match stream.try_clone() {
            Ok(s) => s,
            Err(_) => return,
        });
        let mut writer = stream;
        serve_connection(&mut reader, &mut writer, &token, &broker);
        let _ = writer.shutdown(std::net::Shutdown::Both);
    }

    fn nix_euid() -> u32 {
        unsafe { libc::geteuid() }
    }
}

// =====================
// 测试
// =====================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_git_credential_input() {
        let input = "protocol=https\nhost=github.com\npath=o/r.git\n\nusername=x\n";
        let map = parse_credential_input(input);
        assert_eq!(map.get("protocol").unwrap(), "https");
        assert_eq!(map.get("host").unwrap(), "github.com");
        assert_eq!(map.get("path").unwrap(), "o/r.git");
        // 空行后的字段不属于请求。
        assert!(!map.contains_key("username"));
    }

    #[test]
    fn formats_credential_output() {
        let out = format_credential_output("u", "p");
        assert_eq!(out, "username=u\npassword=p\n\n");
    }

    #[test]
    fn credential_key_shapes() {
        assert_eq!(
            credential_key(Some("https"), "github.com", Some("o/r.git")),
            "https://github.com/o/r.git"
        );
        assert_eq!(
            credential_key(Some("https"), "github.com", Some("/o/r.git")),
            "https://github.com/o/r.git"
        );
        assert_eq!(
            credential_key(Some("ssh"), "gitlab.com", None),
            "ssh://gitlab.com"
        );
        assert_eq!(credential_key(None, "host", None), "host");
    }

    #[test]
    fn classifies_askpass_prompts() {
        let new_host = "The authenticity of host 'github.com ([140.82.121.4]:22)' can't be established.\nED25519 key fingerprint is SHA256:+DiY3wvvV6TuJJhbpZisF/zLDA0zPMSvHdkr4UvCOqU.\nAre you sure you want to continue connecting (yes/no/[fingerprint])? ";
        match classify_askpass(new_host) {
            AskpassKind::HostKeyNew { host, fingerprint } => {
                assert_eq!(host.as_deref(), Some("github.com"));
                assert_eq!(
                    fingerprint,
                    "SHA256:+DiY3wvvV6TuJJhbpZisF/zLDA0zPMSvHdkr4UvCOqU"
                );
            }
            other => panic!("expected HostKeyNew, got {other:?}"),
        }

        let changed = "@@@@@\n@ WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED! @\n@@@@@";
        assert_eq!(classify_askpass(changed), AskpassKind::HostKeyChanged);

        let passphrase = "Enter passphrase for key '/c/Users/x/.ssh/id_ed25519': ";
        match classify_askpass(passphrase) {
            AskpassKind::Secret { key_hint } => {
                assert_eq!(key_hint.as_deref(), Some("/c/Users/x/.ssh/id_ed25519"));
            }
            other => panic!("expected Secret, got {other:?}"),
        }

        let username = "Username for 'https://github.com': ";
        assert!(matches!(classify_askpass(username), AskpassKind::Generic));
    }

    #[test]
    fn token_is_hex_and_unique() {
        let a = random_token();
        let b = random_token();
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    /// broker get 全链路（内存 store + 可编程 UI 桥）：
    /// get（remember=false）→ 会话缓存；store 不落 keychain；
    /// get（remember=true）→ store 落 keychain + 索引；erase 全清；UI 取消。
    #[test]
    fn broker_get_store_erase_flow() {
        /// 可编程 UI 桥：收集 show 事件，由主线程注入应答。
        struct Collect(Arc<Mutex<Vec<CredentialPrompt>>>);
        impl UiBridge for Collect {
            fn show(&self, p: CredentialPrompt) {
                self.0.lock().unwrap().push(p);
            }
        }

        let dir = std::env::temp_dir().join(format!(
            "ibexgit-cred-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let prompts = Arc::new(Mutex::new(Vec::new()));
        let broker = CredentialBroker::with_store(
            dir,
            Arc::new(Collect(prompts.clone())),
            Box::new(MemoryStore::default()),
            "test".to_string(),
        );

        let req = CredRequest {
            id: "r1".into(),
            op: "get".into(),
            protocol: Some("https".into()),
            host: Some("github.com".into()),
            path: Some("/o/r.git".into()),
            username: None,
            secret: None,
            prompt: None,
            ppid: None,
        };
        let wait_prompt =
            |prompts: &Arc<Mutex<Vec<CredentialPrompt>>>, want: usize| -> Vec<CredentialPrompt> {
                for _ in 0..200 {
                    if prompts.lock().unwrap().len() >= want {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                prompts.lock().unwrap().clone()
            };

        // get #1 → UI 提交（remember=false）
        let h = {
            let broker = broker.clone();
            let req = req.clone();
            std::thread::spawn(move || broker.handle(req))
        };
        let ps = wait_prompt(&prompts, 1);
        let CredentialPrompt::Https {
            request_id, host, ..
        } = &ps[0]
        else {
            panic!("expected https prompt");
        };
        assert_eq!(host, "github.com");
        broker
            .respond(
                request_id,
                CredentialReply::Submit {
                    username: Some("octocat".into()),
                    secret: Some("s3cret".into()),
                    remember: false,
                },
            )
            .unwrap();
        let resp = h.join().unwrap();
        assert_eq!(resp.username.as_deref(), Some("octocat"));
        assert_eq!(resp.secret.as_deref(), Some("s3cret"));

        // get #2 → 会话缓存命中，无第二次 UI
        let resp2 = broker.handle(CredRequest {
            id: "r2".into(),
            ..req.clone()
        });
        assert_eq!(resp2.secret.as_deref(), Some("s3cret"));
        assert_eq!(prompts.lock().unwrap().len(), 1);

        // store（git 认证成功）→ remember=false → 不落 keychain
        let store_req = CredRequest {
            id: "r3".into(),
            op: "store".into(),
            protocol: Some("https".into()),
            host: Some("github.com".into()),
            path: Some("/o/r.git".into()),
            username: Some("octocat".into()),
            secret: Some("s3cret".into()),
            prompt: None,
            ppid: None,
        };
        broker.handle(store_req.clone());
        assert!(broker
            .store
            .get("https://github.com/o/r.git")
            .unwrap()
            .is_none());
        assert!(broker.list_index().is_empty());

        // get（remember=true）→ store 落 keychain + 索引。
        // 换一个 path（store 已把凭据写进会话缓存，同 key 的 get 不会再弹 UI）。
        let req2 = CredRequest {
            path: Some("/o/other.git".into()),
            ..req.clone()
        };
        let h = {
            let broker = broker.clone();
            let req = CredRequest {
                id: "r4".into(),
                ..req2.clone()
            };
            std::thread::spawn(move || broker.handle(req))
        };
        let ps = wait_prompt(&prompts, 2);
        let CredentialPrompt::Https { request_id, .. } = &ps[1] else {
            panic!("expected https prompt");
        };
        broker
            .respond(
                request_id,
                CredentialReply::Submit {
                    username: Some("octocat".into()),
                    secret: Some("s3cret".into()),
                    remember: true,
                },
            )
            .unwrap();
        let resp = h.join().unwrap();
        assert_eq!(resp.secret.as_deref(), Some("s3cret"));

        broker.handle(CredRequest {
            id: "r5".into(),
            op: "store".into(),
            username: Some("octocat".into()),
            secret: Some("s3cret".into()),
            ..req2.clone()
        });
        assert!(broker
            .store
            .get("https://github.com/o/other.git")
            .unwrap()
            .is_some());
        assert_eq!(broker.list_index().len(), 1);

        // erase（凭据被远端拒绝）→ keychain + 索引全清
        broker.handle(CredRequest {
            id: "r6".into(),
            op: "erase".into(),
            ..req2
        });
        assert!(broker
            .store
            .get("https://github.com/o/other.git")
            .unwrap()
            .is_none());
        assert!(broker.list_index().is_empty());

        // get → UI 取消 → cancelled（换新 key 避开会话缓存）。
        // helper 将输出 stderr 标记。
        let h = {
            let broker = broker.clone();
            let req = CredRequest {
                id: "r7".into(),
                path: Some("/o/third.git".into()),
                ..req
            };
            std::thread::spawn(move || broker.handle(req))
        };
        let ps = wait_prompt(&prompts, 3);
        let CredentialPrompt::Https { request_id, .. } = &ps[2] else {
            panic!("expected https prompt");
        };
        broker.respond(request_id, CredentialReply::Cancel).unwrap();
        let resp = h.join().unwrap();
        assert_eq!(resp.cancelled, Some(true));
        assert!(!resp.ok);
    }

    #[test]
    fn trust_store_roundtrip() {
        let dir = std::env::temp_dir().join(format!(
            "ibexgit-trust-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let broker = CredentialBroker::with_store(
            dir,
            Arc::new(NoopBridge),
            Box::new(MemoryStore::default()),
            "test".to_string(),
        );
        assert!(!broker.is_trusted("example.com", "SHA256:abc"));
        broker.trust_host("example.com", "SHA256:abc").unwrap();
        assert!(broker.is_trusted("example.com", "SHA256:abc"));
        assert!(!broker.is_trusted("example.com", "SHA256:xyz"));
        assert_eq!(broker.list_known_hosts().len(), 1);
        broker.remove_known_host("example.com").unwrap();
        assert!(broker.list_known_hosts().is_empty());
    }

    struct NoopBridge;
    impl UiBridge for NoopBridge {
        fn show(&self, _p: CredentialPrompt) {}
    }
}
