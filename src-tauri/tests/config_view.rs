//! P10 integration tests: read-only config viewer + commit template against
//! real git repositories (local scope only — never touches the user's global
//! config).

use ibexgit_lib::core::engine::{CliEngine, GitEngine};
use ibexgit_lib::core::runner::GitProcessRunner;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_repo() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ibexgit-p10-{}-{}",
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

fn git(dir: &Path, args: &[&str]) -> String {
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
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// `git init` with one retry (Windows template flake under AV pressure).
fn git_init(dir: &Path) {
    let args = ["init", "-q", "-b", "main"];
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    if !out.status.success() {
        std::thread::sleep(std::time::Duration::from_millis(100));
        let retry = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            retry.status.success(),
            "git init failed: {}",
            String::from_utf8_lossy(&retry.stderr)
        );
    }
}

fn engine() -> Arc<dyn GitEngine> {
    Arc::new(CliEngine::new(GitProcessRunner::new(60), "git"))
}

/// Build an engine whose `git` is a wrapper script that pins
/// `GIT_CONFIG_GLOBAL` to a temp file (and disables the system config), so
/// `config_set_global` tests never touch the real user config.
fn isolated_engine() -> (Arc<dyn GitEngine>, PathBuf) {
    let dir = temp_repo();
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
    let eng = Arc::new(CliEngine::new(
        GitProcessRunner::new(60),
        wrapper.display().to_string(),
    ));
    (eng, cfg)
}

#[tokio::test(flavor = "multi_thread")]
async fn config_local_lists_repo_entries() {
    let dir = temp_repo();
    git_init(&dir);
    git(&dir, &["config", "user.name", "P10"]);
    git(&dir, &["config", "ibexgit.test", "a = b"]);
    let path = dir.display().to_string();

    let entries = engine().config_local(&path).await.unwrap();
    let get = |k: &str| {
        entries
            .iter()
            .find(|e| e.key == k)
            .map(|e| e.value.clone())
            .unwrap_or_else(|| panic!("{k} missing"))
    };
    assert_eq!(get("user.name"), "P10");
    // `=` inside the value survives the -z round trip.
    assert_eq!(get("ibexgit.test"), "a = b");

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test(flavor = "multi_thread")]
async fn commit_template_reads_local_file_and_unset_is_none() {
    let dir = temp_repo();
    git_init(&dir);
    let path = dir.display().to_string();
    let eng = engine();

    // Unset → None (exit 1 is not an error).
    assert!(eng.commit_template(&path).await.unwrap().is_none());

    // Local config, relative path → resolved against the repo root.
    std::fs::write(dir.join("template.txt"), "subject line\n\nbody\n").unwrap();
    git(&dir, &["config", "commit.template", "template.txt"]);
    let tpl = eng.commit_template(&path).await.unwrap().expect("template");
    assert!(tpl.path.ends_with("template.txt"), "{}", tpl.path);
    assert_eq!(tpl.content, "subject line\n\nbody\n");

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test(flavor = "multi_thread")]
async fn config_global_is_readable() {
    // No mutation — just verify the read path works.
    // Use isolated_engine so runners without a real ~/.gitconfig (e.g.
    // GitHub Actions Windows) don't fail on "No such file or directory".
    let (eng, _cfg) = isolated_engine();
    let entries = eng.config_global().await.unwrap();
    for e in &entries {
        assert!(!e.key.is_empty(), "key must not be empty");
        assert!(!e.key.contains('\n'), "key must not contain LF");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn config_set_global_writes_unsets_and_validates() {
    let (eng, cfg) = isolated_engine();

    // Set → the value lands in the isolated global file and reads back.
    eng.config_set_global("user.name", Some("IbexGit Tester"))
        .await
        .unwrap();
    eng.config_set_global("https.proxy", Some("http://127.0.0.1:7890"))
        .await
        .unwrap();
    let entries = eng.config_global().await.unwrap();
    let get = |k: &str| {
        entries
            .iter()
            .find(|e| e.key == k)
            .map(|e| e.value.clone())
            .unwrap_or_else(|| panic!("{k} missing"))
    };
    assert_eq!(get("user.name"), "IbexGit Tester");
    assert_eq!(get("https.proxy"), "http://127.0.0.1:7890");

    // Overwrite: last value wins.
    eng.config_set_global("user.name", Some("second"))
        .await
        .unwrap();
    let entries = eng.config_global().await.unwrap();
    assert_eq!(entries.iter().filter(|e| e.key == "user.name").count(), 1);
    assert_eq!(
        entries
            .iter()
            .find(|e| e.key == "user.name")
            .map(|e| e.value.clone())
            .unwrap(),
        "second"
    );

    // Unset → gone; unset again (missing key, git exit 5) stays Ok.
    eng.config_set_global("user.name", None).await.unwrap();
    let entries = eng.config_global().await.unwrap();
    assert!(entries.iter().all(|e| e.key != "user.name"));
    eng.config_set_global("user.name", None).await.unwrap();

    // Argument injection / malformed keys are rejected before argv.
    for key in ["--unset", "user name", "username", ""] {
        let r = eng.config_set_global(key, Some("x")).await;
        assert!(
            matches!(r, Err(ibexgit_lib::core::error::AppError::Parse { .. })),
            "{key:?} should be a Parse error"
        );
    }

    std::fs::remove_dir_all(cfg.parent().unwrap()).ok();
}

#[tokio::test(flavor = "multi_thread")]
async fn config_local_fails_outside_repo() {
    let dir = temp_repo(); // no git init
    let path = dir.display().to_string();
    let err = engine().config_local(&path).await.unwrap_err();
    assert!(matches!(
        err,
        ibexgit_lib::core::error::AppError::GitCommand { .. }
    ));
    std::fs::remove_dir_all(&dir).ok();
}
