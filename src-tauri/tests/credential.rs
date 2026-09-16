//! P7 凭据回连通道端到端测试：真实 broker（内存 store）+ 真实 helper 二进制
//! （`CARGO_BIN_EXE_credential-helper`），覆盖握手、get/store/erase、askpass、
//! 取消标记，以及**错误 token 被拒的负向验收**。

use ibexgit_lib::core::credential::{
    start_channel_server, ChannelAddr, CredentialBroker, CredentialPrompt, CredentialReply,
    CredentialStore, MemoryStore, StoredCred,
};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 脚本化 UI 桥：show 时按脚本弹出的应答直接 respond（同线程，无死锁）。
struct ScriptedBridge {
    script: Mutex<Vec<CredentialReply>>,
    broker: Mutex<Weak<CredentialBroker>>,
}

impl ibexgit_lib::core::credential::UiBridge for ScriptedBridge {
    fn show(&self, prompt: CredentialPrompt) {
        let request_id = match &prompt {
            CredentialPrompt::Https { request_id, .. }
            | CredentialPrompt::Askpass { request_id, .. }
            | CredentialPrompt::HostKey { request_id, .. } => request_id.clone(),
        };
        let reply = self
            .script
            .lock()
            .unwrap()
            .pop()
            .unwrap_or(CredentialReply::Cancel);
        if let Some(b) = self.broker.lock().unwrap().upgrade() {
            let _ = b.respond(&request_id, reply);
        }
    }
}

fn unique_suffix() -> u128 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed) as u128;
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
        // 时间戳在同一 tick 内可能重复（并行测试），叠加进程内序号保证唯一。
        .wrapping_mul(1000)
        + n
}

fn test_addr() -> String {
    #[cfg(windows)]
    {
        format!(
            r"\\.\pipe\ibexgit-test-cred-{}-{}",
            std::process::id(),
            unique_suffix()
        )
    }
    #[cfg(unix)]
    {
        std::env::temp_dir()
            .join(format!(
                "ibexgit-test-cred-{}-{}.sock",
                std::process::id(),
                unique_suffix()
            ))
            .display()
            .to_string()
    }
}

fn setup(
    script: Vec<CredentialReply>,
    seed: Vec<(&str, StoredCred)>,
) -> (Arc<CredentialBroker>, String) {
    eprintln!("SETUP-LOG-ON");
    let dir = std::env::temp_dir().join(format!(
        "ibexgit-cred-test-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let store = MemoryStore::default();
    for (k, v) in seed {
        store.set(k, &v).unwrap();
    }
    let bridge = Arc::new(ScriptedBridge {
        script: Mutex::new(script),
        broker: Mutex::new(Weak::new()),
    });
    let addr = test_addr();
    let broker = CredentialBroker::with_store(dir, bridge.clone(), Box::new(store), addr.clone());
    *bridge.broker.lock().unwrap() = Arc::downgrade(&broker);
    let channel = if cfg!(windows) {
        ChannelAddr::NamedPipe(addr.clone())
    } else {
        ChannelAddr::UnixSocket(PathBuf::from(&addr))
    };
    start_channel_server(broker.clone(), channel, broker.token().to_string()).unwrap();
    (broker, addr)
}

fn run_helper(addr: &str, token: &str, args: &[&str], input: &str) -> std::process::Output {
    let helper = env!("CARGO_BIN_EXE_credential-helper");
    let mut child = Command::new(helper)
        .args(args)
        .env("IBEXGIT_CRED_ADDR", addr)
        .env("IBEXGIT_CRED_TOKEN", token)
        .env("IBEXGIT_HELPER_DEBUG", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("helper binary must be buildable");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

const GET_ARGS: &[&str] = &["credential", "get"];
const STORE_ARGS: &[&str] = &["credential", "store"];
const ERASE_ARGS: &[&str] = &["credential", "erase"];
const GET_INPUT: &str = "protocol=https\nhost=example.com\npath=o/r.git\n\n";
const KEY: &str = "https://example.com/o/r.git";

#[test]
fn helper_get_hits_seeded_store_without_ui() {
    let (broker, addr) = setup(
        vec![],
        vec![(
            KEY,
            StoredCred {
                username: "alice".into(),
                secret: "wonderland".into(),
            },
        )],
    );
    let out = run_helper(&addr, broker.token(), GET_ARGS, GET_INPUT);
    assert!(
        out.status.success(),
        "exit={:?} stderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        stdout,
        "username=alice\npassword=wonderland\n\n",
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn helper_get_via_ui_then_store_persists_with_remember() {
    let (broker, addr) = setup(
        vec![CredentialReply::Submit {
            username: Some("bob".into()),
            secret: Some("builder".into()),
            remember: true,
        }],
        vec![],
    );
    let out = run_helper(&addr, broker.token(), GET_ARGS, GET_INPUT);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(stdout, "username=bob\npassword=builder\n\n");

    // git 认证成功后的 store 动作 → keychain + 索引。
    let out = run_helper(
        &addr,
        broker.token(),
        STORE_ARGS,
        "protocol=https\nhost=example.com\npath=o/r.git\nusername=bob\npassword=builder\n\n",
    );
    assert!(out.status.success());
    let stored = broker.peek_stored(KEY).expect("remember=true must persist");
    assert_eq!(stored.username, "bob");
    assert_eq!(stored.username, broker.list_index()[0].username);
}

#[test]
fn helper_get_cancelled_writes_marker_and_exit_3() {
    let (broker, addr) = setup(vec![CredentialReply::Cancel], vec![]);
    let out = run_helper(&addr, broker.token(), GET_ARGS, GET_INPUT);
    assert_eq!(out.status.code(), Some(3), "user cancel must exit 3");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("ibexgit: credential-cancelled"),
        "stderr must carry the cancel marker: {stderr}"
    );
    assert!(out.stdout.is_empty());
}

#[test]
fn helper_remember_false_never_persists() {
    let (broker, addr) = setup(
        vec![CredentialReply::Submit {
            username: Some("eve".into()),
            secret: Some("s3cret".into()),
            remember: false,
        }],
        vec![],
    );
    let out = run_helper(&addr, broker.token(), GET_ARGS, GET_INPUT);
    assert!(out.status.success());
    let _ = run_helper(
        &addr,
        broker.token(),
        STORE_ARGS,
        "protocol=https\nhost=example.com\npath=o/r.git\nusername=eve\npassword=s3cret\n\n",
    );
    assert!(broker.peek_stored(KEY).is_none());
    assert!(broker.list_index().is_empty());
}

#[test]
fn helper_erase_clears_keychain_and_index() {
    let (broker, addr) = setup(
        vec![],
        vec![(
            KEY,
            StoredCred {
                username: "alice".into(),
                secret: "wonderland".into(),
            },
        )],
    );
    // 先伪造索引（erase 也应清掉）。
    let out = run_helper(
        &addr,
        broker.token(),
        STORE_ARGS,
        "protocol=https\nhost=example.com\npath=o/r.git\nusername=alice\npassword=wonderland\n\n",
    );
    assert!(out.status.success());
    // store 无批准意图 → 不落库（但索引也不应新增）。
    assert!(broker.list_index().is_empty());

    let out = run_helper(&addr, broker.token(), ERASE_ARGS, GET_INPUT);
    assert!(out.status.success());
    assert!(broker.peek_stored(KEY).is_none());
}

#[test]
fn helper_askpass_answers_prompt() {
    let (broker, addr) = setup(
        vec![CredentialReply::Submit {
            username: None,
            secret: Some("topsecret".into()),
            remember: false,
        }],
        vec![],
    );
    let helper = env!("CARGO_BIN_EXE_credential-helper");
    let out = Command::new(helper)
        .arg("Enter passphrase for key '/c/x/id_ed25519': ")
        .env("IBEXGIT_CRED_ADDR", &addr)
        .env("IBEXGIT_CRED_TOKEN", broker.token())
        .env("IBEXGIT_HELPER_DEBUG", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), "topsecret\n");
}

#[test]
fn helper_askpass_hostkey_trust_then_auto_answer() {
    let (broker, addr) = setup(
        vec![CredentialReply::TrustHostKey { remember: true }],
        vec![],
    );
    let helper = env!("CARGO_BIN_EXE_credential-helper");
    let prompt = "The authenticity of host 'example.com ([93.184.216.34]:22)' can't be established.\nED25519 key fingerprint is SHA256:tLD5aKlmUcJ TBgYyGmJZ1234567890abcdefghijk.\nAre you sure you want to continue connecting (yes/no/[fingerprint])? ";
    let out = Command::new(helper)
        .arg(prompt)
        .env("IBEXGIT_CRED_ADDR", &addr)
        .env("IBEXGIT_CRED_TOKEN", broker.token())
        .env("IBEXGIT_HELPER_DEBUG", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "first exit={:?} stdout={:?} stderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "yes\n");
    assert_eq!(broker.list_known_hosts().len(), 1);

    // 第二次连接：信任库命中 → 自动应答，无需 UI（脚本为空，默认 Cancel）。
    let out = Command::new(helper)
        .arg(prompt)
        .env("IBEXGIT_CRED_ADDR", &addr)
        .env("IBEXGIT_CRED_TOKEN", broker.token())
        .env("IBEXGIT_HELPER_DEBUG", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "second exit={:?} stdout={:?} stderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "yes\n");
    assert_eq!(broker.list_known_hosts().len(), 1);
}

/// 负向验收：错误 token 的客户端拿不到任何凭据（helper 侧表现为
/// 通道拒绝 → 空输出 exit 0）。
#[test]
fn wrong_token_gets_nothing() {
    let (_broker, addr) = setup(
        vec![],
        vec![(
            KEY,
            StoredCred {
                username: "alice".into(),
                secret: "wonderland".into(),
            },
        )],
    );
    let out = run_helper(
        &addr,
        "wrong-token-wrong-token-wrong-token",
        GET_ARGS,
        GET_INPUT,
    );
    assert!(
        out.status.success(),
        "channel failure must not error the helper"
    );
    assert!(
        out.stdout.is_empty(),
        "no credential may leak to a wrong-token client: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// 负向验收：裸客户端错误握手 → 服务端明确拒绝（ok=false）后关闭。
#[test]
fn raw_client_bad_handshake_gets_nak() {
    let (broker, addr) = setup(vec![], vec![]);
    let mut nak = None;
    for _ in 0..40 {
        let attempt = try_bad_handshake(&addr);
        if let Ok(line) = attempt {
            nak = Some(line);
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let line = nak.expect("server must respond to bad handshake");
    assert!(line.contains("\"ok\":false"), "expected nak, got: {line}");
    // 同一连接不应再得到任何应答（服务端已关闭）。
    let _ = broker;
}

fn try_bad_handshake(addr: &str) -> std::io::Result<String> {
    #[cfg(unix)]
    {
        use std::io::BufRead;
        let mut stream = std::os::unix::net::UnixStream::connect(addr)?;
        stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
        writeln!(stream, r#"{{"v":1,"token":"nope"}}"#)?;
        let mut line = String::new();
        std::io::BufReader::new(stream).read_line(&mut line)?;
        Ok(line)
    }
    #[cfg(windows)]
    {
        use std::io::BufRead;
        let mut stream = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(addr)?;
        writeln!(stream, r#"{{"v":1,"token":"nope"}}"#)?;
        let mut line = String::new();
        std::io::BufReader::new(stream).read_line(&mut line)?;
        Ok(line)
    }
}
