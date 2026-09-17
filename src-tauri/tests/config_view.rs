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
    // No mutation — just verify the read path works in this environment.
    let entries = engine().config_global().await.unwrap();
    for e in &entries {
        assert!(!e.key.is_empty(), "key must not be empty");
        assert!(!e.key.contains('\n'), "key must not contain LF");
    }
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
