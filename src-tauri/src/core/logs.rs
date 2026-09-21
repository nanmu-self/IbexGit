//! 日志文件保留策略：`tracing_appender::rolling::daily` 只会按天累积
//! `ibexgit.log.YYYY-MM-DD`，从不删除。这里在启动时做一次廉价清理，
//! 删掉超出保留窗口的旧文件。只认 `ibexgit.log.<date>` 命名模式，
//! 其他文件（用户手动放的、非本 appender 命名的）一律不碰。

use std::path::Path;

/// 日志文件名前缀（与 `lib.rs::init_tracing` 的 daily appender 共用）。
/// tracing-appender 按 UTC 日期生成 `{prefix}.YYYY-MM-DD`。
pub const LOG_FILE_PREFIX: &str = "ibexgit.log";

/// 默认保留天数：最近 14 天的日志文件保留，更早的删除。
pub const LOG_KEEP_DAYS: u32 = 14;

/// 解析 appender 生成的日期后缀（`ibexgit.log.2026-02-10` → 该日期）。
/// 不匹配命名的文件返回 `None`（保留）。
pub fn dated_log_file(name: &str) -> Option<chrono::NaiveDate> {
    name.strip_prefix(LOG_FILE_PREFIX)?
        .strip_prefix('.')?
        .parse()
        .ok()
        .map(|d: chrono::NaiveDate| d)
}

/// 删除 `dir` 下日期早于 `today - keep_days` 的日志文件。
/// 单个文件删除失败（如仍被占用）按 best-effort 跳过，只统计成功数；
/// 只有目录本身不可读才返回 `Err`。返回删除的文件数。
pub fn prune_old_logs(
    dir: &Path,
    today: chrono::NaiveDate,
    keep_days: u32,
) -> std::io::Result<usize> {
    let cutoff = today - chrono::Duration::days(i64::from(keep_days));
    let mut removed = 0;
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        // 目录不递归也不删除；非本 appender 命名的文件不动。
        if entry.file_type()?.is_dir() {
            continue;
        }
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let Some(date) = dated_log_file(&name) else {
            continue;
        };
        if date < cutoff {
            // 删除失败（占用/权限）不影响其余文件，继续清。
            if std::fs::remove_file(entry.path()).is_ok() {
                removed += 1;
            }
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_logs_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ibexgit-logs-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn touch(dir: &Path, name: &str) {
        std::fs::write(dir.join(name), b"x").unwrap();
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    #[test]
    fn parses_only_dated_names() {
        let d = chrono::NaiveDate::from_ymd_opt(2026, 2, 10).unwrap();
        assert_eq!(dated_log_file("ibexgit.log.2026-02-10"), Some(d));
        assert_eq!(dated_log_file("ibexgit.log"), None);
        assert_eq!(dated_log_file("ibexgit.log.not-a-date"), None);
        assert_eq!(dated_log_file("other.txt"), None);
        assert_eq!(dated_log_file("ibexgit.log."), None);
    }

    #[test]
    fn prunes_only_expired_dated_files() {
        let dir = temp_logs_dir("prune");
        let today = chrono::NaiveDate::from_ymd_opt(2026, 2, 10).unwrap();
        touch(&dir, "ibexgit.log.2026-01-11"); // 30 天前 → 删
        touch(&dir, "ibexgit.log.2026-01-27"); // 恰好等于 today-14 → 保留
        touch(&dir, "ibexgit.log.2026-02-09"); // 昨天 → 保留
        touch(&dir, "ibexgit.log.2026-02-10"); // 今天 → 保留
        touch(&dir, "ibexgit.log"); // 无日期后缀 → 不碰
        touch(&dir, "ibexgit.log.not-a-date"); // 不可解析 → 不碰
        touch(&dir, "other.txt"); // 外来文件 → 不碰
        std::fs::create_dir_all(dir.join("subdir")).unwrap();

        let removed = prune_old_logs(&dir, today, LOG_KEEP_DAYS).unwrap();
        assert_eq!(removed, 1);
        assert_eq!(
            names(&dir),
            [
                "ibexgit.log",
                "ibexgit.log.2026-01-27",
                "ibexgit.log.2026-02-09",
                "ibexgit.log.2026-02-10",
                "ibexgit.log.not-a-date",
                "other.txt",
                "subdir",
            ]
        );
    }

    #[test]
    fn empty_and_missing_dirs() {
        let empty = temp_logs_dir("empty");
        assert_eq!(
            prune_old_logs(&empty, chrono::Utc::now().date_naive(), 14).unwrap(),
            0
        );
        let missing = empty.join("does-not-exist");
        assert!(prune_old_logs(&missing, chrono::Utc::now().date_naive(), 14).is_err());
    }
}
