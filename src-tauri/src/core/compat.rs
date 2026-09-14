use crate::core::error::AppError;
use regex::Regex;
use std::process::Command;

bitflags::bitflags! {
    /// Git capability flags derived from `git --version` and feature probing.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, serde::Serialize, serde::Deserialize)]
    pub struct GitCapabilities: u32 {
        /// `git status --porcelain=v2` is supported (Git 2.11+)
        const PORCELAIN_V2 = 1 << 0;
        /// `git restore` / `git switch` are available (Git 2.23+)
        const RESTORE_SWITCH = 1 << 1;
        /// `git push --set-upstream` works (all supported versions, kept for completeness)
        const PUSH_SET_UPSTREAM = 1 << 2;
        /// `git push --force-with-lease` is supported
        const FORCE_WITH_LEASE = 1 << 3;
        /// `-c core.quotepath=false` reliably works (all supported versions)
        const QUOTEPATH_FALSE = 1 << 4;
        /// `git log --topo-order` is available (all supported versions)
        const TOPO_ORDER = 1 << 5;
        /// `git diff --name-only` works (all supported versions)
        const DIFF_NAME_ONLY = 1 << 6;
        /// `git stash` is available (all supported versions)
        const STASH = 1 << 7;
        /// `git reflog` is available (all supported versions)
        const REFLOG = 1 << 8;
        /// `git merge-base` is available (all supported versions)
        const MERGE_BASE = 1 << 9;
        /// `git worktree` is available (Git 2.5+)
        const WORKTREE = 1 << 10;
        /// `git submodule` commands are available (all supported versions)
        const SUBMODULE = 1 << 11;
        /// `git diff --submodule` is available (all supported versions)
        const DIFF_SUBMODULE = 1 << 12;
        /// `git log --follow` supports `--` separator (all supported versions)
        const LOG_FOLLOW = 1 << 13;
        /// `git config --global` / `--local` work as expected (all supported versions)
        const CONFIG_GLOBAL_LOCAL = 1 << 14;
        /// `git remote set-url` is available (all supported versions)
        const REMOTE_SET_URL = 1 << 15;
        /// `git tag -a` / `-s` are available (all supported versions)
        const TAG_ANNOTATED = 1 << 16;
        /// `git branch --show-current` is available (Git 2.22+)
        const BRANCH_SHOW_CURRENT = 1 << 17;
        /// `git rev-parse --verify` works (all supported versions)
        const REV_PARSE_VERIFY = 1 << 18;
        /// `git apply --cached` / `--reverse` are available (all supported versions)
        const APPLY_CACHED_REVERSE = 1 << 19;
        /// `git diff -z` (null-terminated) is available (all supported versions)
        const DIFF_Z = 1 << 20;
        /// `git status --untracked-files=all` works (all supported versions)
        const STATUS_UNTRACKED_ALL = 1 << 21;
        /// `git merge --strategy-option` is available (all supported versions)
        const MERGE_STRATEGY_OPTION = 1 << 22;
        /// `git rebase --onto` is available (all supported versions)
        const REBASE_ONTO = 1 << 23;
        /// `git cherry-pick` is available (all supported versions)
        const CHERRY_PICK = 1 << 24;
        /// `git reset --mixed` / `--soft` / `--hard` are available (all supported versions)
        const RESET_MODES = 1 << 25;
        /// `git clean -n` (preview) is available (all supported versions)
        const CLEAN_PREVIEW = 1 << 26;
        /// `git config --list` works (all supported versions)
        const CONFIG_LIST = 1 << 27;
        /// `git config --get` works (all supported versions)
        const CONFIG_GET = 1 << 28;
        /// `git config --add` works (all supported versions)
        const CONFIG_ADD = 1 << 29;
        /// `git blame --porcelain` is available (all supported versions)
        const BLAME_PORCELAIN = 1 << 30;
    }
}

impl GitCapabilities {
    /// Detect capabilities from the current `git --version` output.
    /// Minimum supported version is 2.40.0.
    pub fn detect() -> Result<Self, AppError> {
        let output = Command::new("git")
            .arg("--version")
            .output()
            .map_err(|e| AppError::io_with_detail("git --version", e.to_string()))?;

        if !output.status.success() {
            return Err(AppError::git_command(
                "git --version",
                String::from_utf8_lossy(&output.stderr),
                String::from_utf8_lossy(&output.stdout),
            ));
        }

        let version_str = String::from_utf8_lossy(&output.stdout);
        let version = Self::parse_version(&version_str)?;

        if version < (2, 40, 0) {
            return Err(AppError::GitVersionTooOld {
                found: version_str.trim().to_string(),
                required: "2.40.0".to_string(),
            });
        }

        let mut caps = Self::empty();

        // Git 2.40+ supports all capabilities we need for MVP
        caps.insert(Self::PORCELAIN_V2);
        caps.insert(Self::RESTORE_SWITCH);
        caps.insert(Self::PUSH_SET_UPSTREAM);
        caps.insert(Self::FORCE_WITH_LEASE);
        caps.insert(Self::QUOTEPATH_FALSE);
        caps.insert(Self::TOPO_ORDER);
        caps.insert(Self::DIFF_NAME_ONLY);
        caps.insert(Self::STASH);
        caps.insert(Self::REFLOG);
        caps.insert(Self::MERGE_BASE);
        caps.insert(Self::WORKTREE);
        caps.insert(Self::SUBMODULE);
        caps.insert(Self::DIFF_SUBMODULE);
        caps.insert(Self::LOG_FOLLOW);
        caps.insert(Self::CONFIG_GLOBAL_LOCAL);
        caps.insert(Self::REMOTE_SET_URL);
        caps.insert(Self::TAG_ANNOTATED);
        caps.insert(Self::BRANCH_SHOW_CURRENT);
        caps.insert(Self::REV_PARSE_VERIFY);
        caps.insert(Self::APPLY_CACHED_REVERSE);
        caps.insert(Self::DIFF_Z);
        caps.insert(Self::STATUS_UNTRACKED_ALL);
        caps.insert(Self::MERGE_STRATEGY_OPTION);
        caps.insert(Self::REBASE_ONTO);
        caps.insert(Self::CHERRY_PICK);
        caps.insert(Self::RESET_MODES);
        caps.insert(Self::CLEAN_PREVIEW);
        caps.insert(Self::CONFIG_LIST);
        caps.insert(Self::CONFIG_GET);
        caps.insert(Self::CONFIG_ADD);
        caps.insert(Self::BLAME_PORCELAIN);

        Ok(caps)
    }

    fn parse_version(version_str: &str) -> Result<(u32, u32, u32), AppError> {
        // git version 2.40.0.windows.1
        let re = Regex::new(r"(\d+)\.(\d+)\.(\d+)").unwrap();
        let caps = re.captures(version_str).ok_or_else(|| {
            AppError::parse(format!("Unable to parse git version: {}", version_str))
        })?;

        let major = caps[1]
            .parse::<u32>()
            .map_err(|e| AppError::parse(e.to_string()))?;
        let minor = caps[2]
            .parse::<u32>()
            .map_err(|e| AppError::parse(e.to_string()))?;
        let patch = caps[3]
            .parse::<u32>()
            .map_err(|e| AppError::parse(e.to_string()))?;

        Ok((major, minor, patch))
    }
}
