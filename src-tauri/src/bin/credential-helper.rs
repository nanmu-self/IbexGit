//! credential-helper（P7，独立进程，ADR-004）。
//!
//! 单二进制两种模式（argv 分流）：
//! - **credential 模式**：`credential-helper credential <get|store|erase>`
//!   或 `credential-helper ibexgit-credential <get|store|erase>`（后者是
//!   spawn_injection 注入 `-c credential.helper="<path> ibexgit-credential"`
//!   时 git 实际派生的形式：静态标记 + 追加操作名）。
//!   stdin 是 git credential 协议请求，`get` 时 stdout 输出协议应答。
//! - **askpass 模式**：`credential-helper <提示文本>`
//!   由 `GIT_ASKPASS` / `SSH_ASKPASS` 直接 exec（argv[1] = 提示文本），
//!   stdout 输出一行答案。
//!
//! 两种模式都经本地通道（Windows Named Pipe / UDS）回连主程序的
//! CredentialBroker；连接需 env 注入的握手 token。通道不可用 / 连接失败 →
//! 静默退出 0（get）/ exit 1（askpass），git 按无凭据处理 —— helper 的任何
//! 故障都不允许演变为挂起。
//!
//! 用户取消：stderr 输出 `ibexgit: credential-cancelled` + exit 3（credential
//! get）/ exit 1（askpass）；标记经 git stderr 透传给 Runner 映射
//! `CredentialCancelled`（设计文档 §5）。
//!
//! 本二进制刻意只依赖 serde/serde_json 与主 crate 的协议类型，
//! 不链接 Tauri，保证体积与启动速度。

use ibexgit_lib::core::credential::{
    format_credential_output, parse_credential_input, CredRequest, CredResponse, Hello, HelloAck,
    CREDENTIAL_CANCELLED_MARKER,
};
use std::io::{BufRead, BufReader, Read, Write};
use std::process::ExitCode;
use std::time::Duration;

const CONNECT_ATTEMPTS: u32 = 40;
const CONNECT_RETRY: Duration = Duration::from_millis(200);
/// broker UI 等待 300s + 余量（Windows 无 socket 超时 API，仅 Unix）。
#[cfg(unix)]
const IO_TIMEOUT: Duration = Duration::from_secs(330);

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        // credential 模式：静态标记 + 操作名（两种标记都认，见模块注释）。
        Some("credential") | Some("ibexgit-credential") => {
            credential_mode(args.get(1).map(String::as_str))
        }
        // 其余（argv[0] = 提示文本）→ askpass
        Some(_) => askpass_mode(&args[0]),
        None => ExitCode::from(2),
    }
}

/// credential helper 协议模式。action：get / store / erase。
fn credential_mode(action: Option<&str>) -> ExitCode {
    let Some(action) = action else {
        return ExitCode::from(2);
    };
    let op = match action {
        "get" | "store" | "erase" => action,
        _ => return ExitCode::from(2),
    };

    // stdin 请求（空行结束；read_to_string 更宽容，git 会关闭 stdin）。
    let stdin = std::io::stdin();
    let mut input = String::new();
    if stdin.lock().read_to_string(&mut input).is_err() {
        return ExitCode::from(2);
    }
    let fields = parse_credential_input(&input);

    let req = CredRequest {
        id: format!("h{}", request_nonce()),
        op: op.to_string(),
        protocol: non_empty(fields.get("protocol")),
        host: non_empty(fields.get("host")),
        path: non_empty(fields.get("path")),
        username: non_empty(fields.get("username")),
        secret: non_empty(fields.get("password")),
        prompt: None,
        ppid: parent_pid(),
    };

    let Some(resp) = roundtrip(&req) else {
        // 通道不可用：git 视为 helper 未命中（exit 0 + 空输出），
        // GIT_TERMINAL_PROMPT=0 下 git 会以"无法获取凭据"失败 —— 不挂起。
        return ExitCode::SUCCESS;
    };

    match action {
        "get" => {
            if resp.cancelled == Some(true) {
                eprintln!("{CREDENTIAL_CANCELLED_MARKER}");
                return ExitCode::from(3);
            }
            if let (Some(u), Some(p)) = (resp.username, resp.secret) {
                let mut stdout = std::io::stdout();
                let _ = stdout.write_all(format_credential_output(&u, &p).as_bytes());
                let _ = stdout.flush();
            }
            ExitCode::SUCCESS
        }
        _ => ExitCode::SUCCESS,
    }
}

/// askpass 模式：提示文本 → 通道 → 单行答案。
fn askpass_mode(prompt: &str) -> ExitCode {
    let req = CredRequest {
        id: format!("a{}", request_nonce()),
        op: "askpass".into(),
        protocol: None,
        host: None,
        path: None,
        username: None,
        secret: None,
        prompt: Some(prompt.to_string()),
        ppid: parent_pid(),
    };
    match roundtrip(&req) {
        Some(resp) if resp.cancelled != Some(true) => {
            let answer = resp.answer.or(resp.secret).unwrap_or_default();
            let mut stdout = std::io::stdout();
            let _ = stdout.write_all(answer.as_bytes());
            let _ = stdout.write_all(b"\n");
            let _ = stdout.flush();
            ExitCode::SUCCESS
        }
        _ => ExitCode::from(1),
    }
}

fn parent_pid() -> Option<u32> {
    std::env::var("IBEXGIT_PPID")
        .ok()
        .and_then(|p| p.parse().ok())
}

fn request_nonce() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

fn non_empty(v: Option<&String>) -> Option<String> {
    v.filter(|s| !s.is_empty()).cloned()
}

/// 连接 broker 通道：握手 token（env）→ 请求 → 应答。
/// NoListener（无实例监听）重试数次后放弃；其它错误立即放弃。
fn roundtrip(req: &CredRequest) -> Option<CredResponse> {
    let dbg = std::env::var("IBEXGIT_HELPER_DEBUG").is_ok();
    let addr = match std::env::var("IBEXGIT_CRED_ADDR") {
        Ok(a) => a,
        Err(e) => {
            if dbg {
                eprintln!("HELPER-DIAG: addr missing: {e}");
            }
            return None;
        }
    };
    let token = match std::env::var("IBEXGIT_CRED_TOKEN") {
        Ok(t) => t,
        Err(e) => {
            if dbg {
                eprintln!("HELPER-DIAG: token missing: {e}");
            }
            return None;
        }
    };
    if dbg {
        eprintln!("HELPER-DIAG: roundtrip op={} addr={addr}", req.op);
    }
    for _ in 0..CONNECT_ATTEMPTS {
        match single_attempt(&addr, &token, req) {
            Ok(resp) => return Some(resp),
            Err(ConnectError::NoListener) => std::thread::sleep(CONNECT_RETRY),
            Err(ConnectError::Other) => return None,
        }
    }
    None
}

enum ConnectError {
    NoListener,
    Other,
}

fn single_attempt(
    addr: &str,
    token: &str,
    req: &CredRequest,
) -> Result<CredResponse, ConnectError> {
    let dbg = std::env::var("IBEXGIT_HELPER_DEBUG").is_ok();
    #[cfg(unix)]
    {
        let stream = std::os::unix::net::UnixStream::connect(addr).map_err(|e| {
            if matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) {
                ConnectError::NoListener
            } else {
                ConnectError::Other
            }
        })?;
        let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
        let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
        let writer = stream.try_clone().map_err(|e| {
            if dbg {
                eprintln!("HELPER-DIAG: try_clone err: {e}");
            }
            ConnectError::Other
        })?;
        exchange(BufReader::new(stream), writer, token, req).map_err(|e| {
            if dbg {
                eprintln!("HELPER-DIAG: exchange err: {e}");
            }
            ConnectError::Other
        })
    }
    #[cfg(windows)]
    {
        let stream = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(addr)
            .map_err(|e| {
                if dbg {
                    eprintln!("HELPER-DIAG: open err: {e}");
                }
                match e.kind() {
                    std::io::ErrorKind::NotFound => ConnectError::NoListener,
                    _ => ConnectError::Other,
                }
            })?;
        let writer = stream.try_clone().map_err(|e| {
            if dbg {
                eprintln!("HELPER-DIAG: try_clone err: {e}");
            }
            ConnectError::Other
        })?;
        exchange(BufReader::new(stream), writer, token, req).map_err(|e| {
            if dbg {
                eprintln!("HELPER-DIAG: exchange err: {e}");
            }
            ConnectError::Other
        })
    }
}

/// 一次连接的握手 + 请求 + 应答（JSON Lines）。
fn exchange<R: Read, W: Write>(
    mut reader: BufReader<R>,
    mut writer: W,
    token: &str,
    req: &CredRequest,
) -> std::io::Result<CredResponse> {
    let hello = serde_json::to_string(&Hello {
        v: 1,
        token: token.to_string(),
    })
    .unwrap_or_default();
    writer.write_all(hello.as_bytes())?;
    writer.write_all(b"\n")?;
    writer.flush()?;

    let mut line = String::new();
    reader.read_line(&mut line)?;
    let ack: HelloAck = serde_json::from_str(line.trim())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    if !ack.ok {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "handshake rejected",
        ));
    }

    let body = serde_json::to_string(req)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    writer.write_all(body.as_bytes())?;
    writer.write_all(b"\n")?;
    writer.flush()?;

    let mut line = String::new();
    reader.read_line(&mut line)?;
    if std::env::var("IBEXGIT_HELPER_DEBUG").is_ok() {
        eprintln!("HELPER-DIAG: response line={line:?}");
    }
    serde_json::from_str(line.trim())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}
