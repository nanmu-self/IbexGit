//! 仓库级 SSH 注入覆盖（P7）：fetch/push 走 `run_ssh` 漏斗，
//! `GIT_SSH_COMMAND` 按仓库的 `core.sshCommand` / `ibexgit.sshkey` 解析。
//!
//! 用 PATH 上的 `ssh` 垫片捕获 git 实际执行的 ssh 参数（垫片写 marker 后
//! exit 1，fetch 报错无妨），端到端验证四种解析结果。env 修改只在单个
//! 测试函数内串行进行，避免并行竞争。

use ibexgit_lib::core::engine::{CliEngine, GitEngine};
use ibexgit_lib::core::runner::GitProcessRunner;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ibexgit-ssh-ovr-{tag}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
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

fn git_init(dir: &Path) {
    let args = ["init", "-q", "-b", "main"];
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git init failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[allow(dead_code)]
/// `git` 包装脚本：隔离全局/系统配置，保证解析只看仓库本地配置。
/// 当前 runner 自带 `with_git_config` 直接注入 env，不再需要；保留作备选。
fn isolated_git_wrapper(dir: &Path) -> String {
    let cfg = dir.join("gitconfig");
    std::fs::write(&cfg, "").unwrap();
    let (ext, body) = if cfg!(windows) {
        (
            ".cmd",
            format!(
                "@echo off\r\nset GIT_CONFIG_GLOBAL={}\r\nset GIT_CONFIG_NOSYSTEM=1\r\ngit %*\r\n",
                cfg.display()
            ),
        )
    } else {
        (
            "",
            format!(
                "#!/bin/sh\nexport GIT_CONFIG_GLOBAL='{}'\nexport GIT_CONFIG_NOSYSTEM=1\nexec git \"$@\"\n",
                cfg.display()
            ),
        )
    };
    let wrapper = dir.join(format!("git-wrap{ext}"));
    std::fs::write(&wrapper, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    wrapper.display().to_string()
}

/// `ssh` 垫片：把参数原样写入 `$IBEXGIT_SSH_MARKER` 环境变量指定的路径，
/// 然后 exit 1（fetch 因此报错，无妨）。
///
/// 所有平台都用无扩展名 + shebang 的 sh 脚本：git 总是经 `sh -c` 执行
/// GIT_SSH_COMMAND / core.sshCommand，msys sh（Windows）按 shebang 识别
/// 可执行文件；.cmd 垫片不会被 sh 的 PATH 搜索命中。
///
/// marker 路径不硬编码在 shim 里，而是通过 env var 传入——这样：
/// - 不依赖 macOS `/tmp` 和 `$TMPDIR` 映射一致（两者在 macOS 上完全不同）
/// - 不依赖 msys sh.exe 正确处理 Windows 8.3 短路径
/// - 跨平台（Win/macOS/Linux）统一可靠
fn write_ssh_shim(dir: &Path) {
    let body = "#!/bin/sh\nprintf '%s\\n' \"$@\" >> \"$IBEXGIT_SSH_MARKER\"\nexit 1\n";
    let shim = dir.join("ssh");
    std::fs::write(&shim, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn fetch_resolves_ssh_override_per_repo() {
    let work = temp_dir("work");
    let shims = temp_dir("shims");
    git_init(&work);
    git(
        &work,
        &["remote", "add", "origin", "git@example.invalid:foo/bar.git"],
    );

    // marker 放在测试自己创建的 work 目录里（不是系统 temp），
    // 路径跨平台一致、短、不会触发 Windows 8.3 短路径名。
    let marker = work.join("ssh-marker.txt");
    let marker_str = marker.display().to_string();

    write_ssh_shim(&shims);
    std::fs::remove_file(&marker).ok();

    // PATH 前插垫片目录：git 子进程解析出的 ssh 即垫片。
    let old_path = std::env::var_os("PATH").unwrap();
    let mut new_path = std::env::split_paths(&shims).collect::<Vec<_>>();
    new_path.extend(std::env::split_paths(&old_path));
    std::env::set_var("PATH", std::env::join_paths(&new_path).unwrap());

    // 告诉 shim 把参数写到哪里（通过 env var 传入完整平台原生路径）。
    std::env::set_var("IBEXGIT_SSH_MARKER", &marker_str);

    // 全局活动密钥（带空格路径：同时验证 runner 侧的引号处理）。
    // runner 自注入 GIT_CONFIG_GLOBAL/NOSYSTEM，绕开 .cmd wrapper 中间层。
    let cfg = work.join("gitconfig");
    std::fs::write(&cfg, "").unwrap();
    let runner = GitProcessRunner::with_git_config(60, cfg.display().to_string());
    runner.net_config().update(|c| {
        c.ssh_key_path = Some("C:\\global dir\\id_global".to_string());
    });
    let eng: Arc<dyn GitEngine> = Arc::new(CliEngine::new(runner, "git"));
    let repo = work.display().to_string();
    let read_marker = || std::fs::read_to_string(&marker).unwrap();

    // 1) 仓库未配置 → Inherit：注入全局活动密钥。
    let _ = eng.fetch(&repo, Some("origin")).await;
    let args = read_marker();
    assert!(
        args.contains("id_global"),
        "case1 expected global key, got: {args}"
    );
    std::fs::write(&marker, "").unwrap();

    // 2) 仓库指定密钥（含空格路径）→ Command：仓库密钥生效，全局被覆盖。
    let repo_key = work.join("my repo key");
    git(
        &work,
        &["config", "ibexgit.sshkey", &repo_key.display().to_string()],
    );
    let _ = eng.fetch(&repo, Some("origin")).await;
    let args = read_marker();
    assert!(
        args.contains("my repo key"),
        "case2 expected repo key, got: {args}"
    );
    assert!(
        !args.contains("id_global"),
        "case2 global key must be shadowed: {args}"
    );
    std::fs::write(&marker, "").unwrap();

    // 3) ibexgit.sshkey="" → Suppress：显式禁用，全局密钥也不得注入。
    git(&work, &["config", "ibexgit.sshkey", ""]);
    let _ = eng.fetch(&repo, Some("origin")).await;
    let args = read_marker();
    assert!(
        !args.contains("IdentitiesOnly") && !args.contains("id_global"),
        "case3 expected no injection, got: {args}"
    );
    std::fs::write(&marker, "").unwrap();

    // 4) 用户自管 core.sshCommand → Suppress：env 不得覆盖用户配置。
    git(&work, &["config", "--unset", "ibexgit.sshkey"]);
    git(&work, &["config", "core.sshCommand", "ssh"]);
    let _ = eng.fetch(&repo, Some("origin")).await;
    let args = read_marker();
    assert!(
        !args.contains("IdentitiesOnly") && !args.contains("id_global"),
        "case4 expected user core.sshCommand honored, got: {args}"
    );

    // 清理
    std::env::set_var("PATH", old_path);
    std::env::remove_var("IBEXGIT_SSH_MARKER");
    std::fs::remove_dir_all(&work).ok();
    std::fs::remove_dir_all(&shims).ok();
}
