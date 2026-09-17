//! P11 AI 管线端到端（ADR-013）：真实仓库采集 → 上下文/隐私过滤 →
//! wiremock Provider → 流式结果。验收路径：
//! - 智能提交消息：staged diff → 采样（排除规则生效）→ 生成；
//! - 日报：小范围单次请求；
//! - 周报 map-reduce：130+ 提交分批摘要再汇总；
//! - 隐私：排除路径不进入任何请求体（对 wiremock 收到的请求断言）。

use ibexgit_lib::core::ai::config::AiConfig;
use ibexgit_lib::core::ai::context::{PathFilter, MAP_REDUCE_COMMIT_THRESHOLD};
use ibexgit_lib::core::ai::generate::{
    self, collect_report, collect_staged, generate_commit_message, generate_report, ReportParams,
};
use ibexgit_lib::core::ai::provider::{AuthStyle, OpenAiProvider};
use ibexgit_lib::core::engine::{CliEngine, DiffSource, GitEngine};
use ibexgit_lib::core::repo::RepoManager;
use ibexgit_lib::core::runner::{GitProcessRunner, SharedNetConfig};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_repo(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ibexgit-ai-{tag}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
            * 1000
            + {
                use std::sync::atomic::Ordering;
                static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                N.fetch_add(1, Ordering::Relaxed) as u128
            }
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("git available");
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
}

fn commit_all(dir: &Path, message: &str) {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "--allow-empty", "-m", message]);
}

fn engine() -> Arc<CliEngine> {
    Arc::new(CliEngine::new(GitProcessRunner::new(60), "git"))
}

fn openai_provider(server_uri: String) -> OpenAiProvider {
    OpenAiProvider::new(
        format!("{server_uri}/chat/completions"),
        "sk-test-key".into(),
        AuthStyle::Bearer,
        "test-model".into(),
        &SharedNetConfig::new(),
        30,
        "wiremock-test".into(),
    )
}

fn sse_body(content: &str) -> String {
    // 单事件 + 终止标记（覆盖解析器对完整流的处理）。
    let payload = serde_json::json!({
        "choices": [{"delta": {"content": content}}]
    });
    format!("data: {payload}\n\ndata: [DONE]\n\n")
}

fn no_status() -> generate::OnStatus {
    Arc::new(|_| {})
}

// =====================
// 智能提交消息
// =====================

#[tokio::test(flavor = "multi_thread")]
async fn commit_message_e2e_with_privacy_exclusions() {
    let dir = temp_repo("msg");
    git(&dir, &["init", "-q", "-b", "main"]);
    git(&dir, &["config", "user.email", "dev@ibexgit.local"]);
    git(&dir, &["config", "user.name", "Dev"]);

    std::fs::write(dir.join("src.rs"), "fn old() {}\n").unwrap();
    commit_all(&dir, "init");
    std::fs::write(dir.join("src.rs"), "fn new() {}\n").unwrap();
    std::fs::write(dir.join("creds.env"), "SECRET=super-secret-value\n").unwrap();
    git(&dir, &["add", "-A"]);

    let mgr = RepoManager::new(engine());
    let path = dir.display().to_string();
    let model = collect_staged(mgr.engine().as_ref(), &mgr, &path)
        .await
        .unwrap();
    assert_eq!(model.files.len(), 2, "both staged files are in the diff");

    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .respond_with(|_: &wiremock::Request| {
            wiremock::ResponseTemplate::new(200).set_body_string(sse_body(
                "feat: rewrite entry function\n\nSwitch fn old to fn new.",
            ))
        })
        .mount(&server)
        .await;

    let cfg = AiConfig::default();
    let filter = PathFilter::new(&cfg.exclude_patterns);
    let provider = openai_provider(server.uri());
    let text = generate_commit_message(
        &provider,
        &model,
        &filter,
        &cfg,
        "zh-CN",
        None,
        Arc::new(|_| {}),
    )
    .await
    .unwrap();
    assert!(text.starts_with("feat:"), "{text}");

    // 隐私断言：请求体包含保留文件与 diff 内容，绝不含被排除路径/内容。
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let body = String::from_utf8_lossy(&requests[0].body).to_string();
    assert!(body.contains("src.rs"), "kept path must be sent: {body}");
    assert!(body.contains("fn new"), "diff content must be sent");
    assert!(
        !body.contains("creds.env"),
        "excluded path must NOT leak: {body}"
    );
    assert!(
        !body.contains("super-secret-value"),
        "excluded file content must NOT leak"
    );
    // 鉴权头只在 Rust 侧使用（wiremock 收到 Bearer）——此处仅验证请求可发出。
    assert_eq!(
        requests[0].headers.get("authorization").unwrap(),
        "Bearer sk-test-key"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test(flavor = "multi_thread")]
async fn commit_message_nothing_staged_fails_clearly() {
    let dir = temp_repo("msg-empty");
    git(&dir, &["init", "-q", "-b", "main"]);
    git(&dir, &["config", "user.email", "dev@ibexgit.local"]);
    git(&dir, &["config", "user.name", "Dev"]);
    std::fs::write(dir.join("a.txt"), "x\n").unwrap();
    commit_all(&dir, "init"); // clean worktree

    let mgr = RepoManager::new(engine());
    let path = dir.display().to_string();
    let model = collect_staged(mgr.engine().as_ref(), &mgr, &path)
        .await
        .unwrap();
    let cfg = AiConfig::default();
    let provider = openai_provider("http://127.0.0.1:9".into());
    let err = generate_commit_message(
        &provider,
        &model,
        &PathFilter::empty(),
        &cfg,
        "en",
        None,
        Arc::new(|_| {}),
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("nothing staged"), "{err}");

    std::fs::remove_dir_all(&dir).ok();
}

// =====================
// 日报（单次请求）
// =====================

#[tokio::test(flavor = "multi_thread")]
async fn daily_report_single_request_e2e() {
    let dir = temp_repo("daily");
    git(&dir, &["init", "-q", "-b", "main"]);
    git(&dir, &["config", "user.email", "dev@ibexgit.local"]);
    git(&dir, &["config", "user.name", "Dev"]);
    for i in 0..5 {
        std::fs::write(dir.join(format!("f{i}.txt")), format!("content {i}\n")).unwrap();
        commit_all(&dir, &format!("feat: add file {i}"));
    }

    let mgr = RepoManager::new(engine());
    let path = dir.display().to_string();
    let sources = vec![("daily-repo".to_string(), path.clone())];
    let since = generate::report_range("daily");
    let commits = collect_report(
        mgr.engine().as_ref(),
        &mgr,
        &sources,
        &since,
        None,
        None,
        no_status(),
    )
    .await
    .unwrap();
    assert_eq!(commits.len(), 5, "all of today's commits are collected");
    assert_eq!(commits[0].repo, "daily-repo", "repo label is attached");
    assert!(
        commits[0].files.iter().any(|f| f.path.contains("f")),
        "numstat attached: {:?}",
        commits[0].files
    );

    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::body_string_contains(
            "Write a daily report",
        ))
        .respond_with(|_: &wiremock::Request| {
            wiremock::ResponseTemplate::new(200)
                .set_body_string(sse_body("## 今日报告\n- 添加 5 个文件"))
        })
        .mount(&server)
        .await;

    let cfg = AiConfig::default();
    let filter = PathFilter::new(&cfg.exclude_patterns);
    let provider = openai_provider(server.uri());
    let md = generate_report(
        &provider,
        &commits,
        &filter,
        &ReportParams {
            kind: "daily".into(),
            since: since.clone(),
            repo_count: 1,
            author_note: "Commits from all authors are included.".into(),
            language: "zh-CN".into(),
        },
        None,
        Arc::new(|_| {}),
        no_status(),
    )
    .await
    .unwrap();
    assert!(md.contains("今日报告"), "{md}");

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1, "small range → single request");
    let body = String::from_utf8_lossy(&requests[0].body).to_string();
    assert!(body.contains("feat: add file"), "commit subjects are sent");
    assert!(
        !body.contains("content {i}"),
        "file contents are never sent (numstat only)"
    );
    assert!(body.contains("f0.txt +1 -0"), "stats are sent: {body}");

    std::fs::remove_dir_all(&dir).ok();
}

// =====================
// 周报 map-reduce（130+ 提交）
// =====================

#[tokio::test(flavor = "multi_thread")]
async fn weekly_report_map_reduce_e2e() {
    let dir = temp_repo("weekly");
    git(&dir, &["init", "-q", "-b", "main"]);
    git(&dir, &["config", "user.email", "dev@ibexgit.local"]);
    git(&dir, &["config", "user.name", "Dev"]);

    // MAP_REDUCE_COMMIT_THRESHOLD + 20 个提交，其中一个改动被排除路径。
    let total = MAP_REDUCE_COMMIT_THRESHOLD + 20;
    for i in 0..total {
        if i % 25 == 0 {
            std::fs::write(dir.join(".env"), "DB_PASSWORD=topsecret\n").unwrap();
        } else {
            std::fs::write(dir.join(format!("m{i}.rs")), format!("// mod {i}\n")).unwrap();
        }
        commit_all(&dir, &format!("feat: change {i}"));
    }

    let mgr = RepoManager::new(engine());
    let path = dir.display().to_string();
    let sources = vec![("weekly-repo".to_string(), path.clone())];
    let since = generate::report_range("weekly");
    let commits = collect_report(
        mgr.engine().as_ref(),
        &mgr,
        &sources,
        &since,
        None,
        None,
        no_status(),
    )
    .await
    .unwrap();
    assert_eq!(commits.len(), total);

    let cfg = AiConfig::default();
    let filter = PathFilter::new(&cfg.exclude_patterns);

    let server = wiremock::MockServer::start().await;
    // map 请求（map_user 含 "Summarize these commits"）→ 批摘要。
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::body_string_contains(
            "Summarize these commits",
        ))
        .respond_with(|_: &wiremock::Request| {
            wiremock::ResponseTemplate::new(200).set_body_string(sse_body("- batch summary bullet"))
        })
        .mount(&server)
        .await;
    // reduce 请求（reduce_user 含 "Merge the following batch summaries"）→ 报告。
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::body_string_contains(
            "Merge the following batch summaries",
        ))
        .respond_with(|_: &wiremock::Request| {
            wiremock::ResponseTemplate::new(200)
                .set_body_string(sse_body("## 本周报告\n- 主题 A\n- 主题 B"))
        })
        .mount(&server)
        .await;

    let provider = openai_provider(server.uri());
    let md = generate_report(
        &provider,
        &commits,
        &filter,
        &ReportParams {
            kind: "weekly".into(),
            since: since.clone(),
            repo_count: 1,
            author_note: String::new(),
            language: "zh-CN".into(),
        },
        None,
        Arc::new(|_| {}),
        no_status(),
    )
    .await
    .unwrap();
    assert!(
        md.contains("本周报告"),
        "final text comes from the reduce call: {md}"
    );

    let requests = server.received_requests().await.unwrap();
    let maps = requests
        .iter()
        .filter(|r| String::from_utf8_lossy(&r.body).contains("Summarize these commits"))
        .count();
    let reduces = requests
        .iter()
        .filter(|r| {
            String::from_utf8_lossy(&r.body).contains("Merge the following batch summaries")
        })
        .count();
    assert!(
        maps >= 2,
        "130+ commits must split into ≥2 map batches, got {maps}"
    );
    assert_eq!(reduces, 1, "exactly one reduce call");

    // 隐私：被排除路径不进任何请求体。
    for r in &requests {
        let body = String::from_utf8_lossy(&r.body);
        assert!(
            !body.contains(".env"),
            "excluded path must not leak: {body}"
        );
        assert!(
            !body.contains("topsecret"),
            "excluded content must not leak"
        );
    }
    // map 请求携带原始提交主题；reduce 只带批摘要。
    let map_bodies: Vec<String> = requests
        .iter()
        .filter(|r| String::from_utf8_lossy(&r.body).contains("Summarize these commits"))
        .map(|r| String::from_utf8_lossy(&r.body).to_string())
        .collect();
    assert!(
        map_bodies.iter().all(|b| b.contains("feat: change")),
        "map requests carry subjects"
    );

    std::fs::remove_dir_all(&dir).ok();
}

// =====================
// diff 读取（staged diff 走 GitEngine，冒烟）
// =====================

#[tokio::test(flavor = "multi_thread")]
async fn staged_diff_source_is_worktree_compatible() {
    let dir = temp_repo("diff");
    git(&dir, &["init", "-q", "-b", "main"]);
    git(&dir, &["config", "user.email", "dev@ibexgit.local"]);
    git(&dir, &["config", "user.name", "Dev"]);
    std::fs::write(dir.join("a.txt"), "one\n").unwrap();
    commit_all(&dir, "init");
    std::fs::write(dir.join("a.txt"), "two\n").unwrap();
    git(&dir, &["add", "a.txt"]);

    let eng = engine();
    let path = dir.display().to_string();
    let model = eng
        .diff(&path, DiffSource::Staged, None, &[], Default::default())
        .await
        .unwrap();
    assert_eq!(model.files.len(), 1);
    assert!(
        model.id == 0,
        "raw engine diff is not cached (command layer caches)"
    );

    std::fs::remove_dir_all(&dir).ok();
}
