//! OS-level "open a terminal at a directory" (P3 工具栏：在终端打开).
//!
//! Not a git operation — lives outside the GitEngine, same layer as
//! `compat`/`proctree`. Per platform we keep an ordered candidate list and
//! spawn the first program that starts successfully. Spawning is
//! fire-and-forget: the `Child` handle is dropped, so the terminal
//! outlives IbexGit and is never killed by us.

use std::io;
use std::path::Path;
use std::process::Command;

/// Launch the user's terminal with `dir` as the working directory.
pub fn launch(dir: &Path) -> io::Result<()> {
    if !dir.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("not a directory: {}", dir.display()),
        ));
    }

    let mut tried: Vec<String> = Vec::new();
    let mut last: Option<io::Error> = None;
    for mut cmd in candidates(dir) {
        let program = cmd.get_program().to_string_lossy().into_owned();
        match cmd.spawn() {
            Ok(_) => {
                tracing::debug!(%program, dir = %dir.display(), "terminal launched");
                return Ok(());
            }
            Err(e) => {
                tracing::debug!(%program, error = %e, "terminal candidate unavailable");
                tried.push(program);
                last = Some(e);
            }
        }
    }

    let tried_msg = if tried.is_empty() {
        "?".to_string()
    } else {
        tried.join(", ")
    };
    Err(io::Error::new(
        last.map(|e| e.kind()).unwrap_or(io::ErrorKind::NotFound),
        format!("no terminal could be launched (tried: {tried_msg})"),
    ))
}

/// Ordered launch candidates for the current platform. Every candidate
/// either receives the directory as an argument or inherits it through
/// `current_dir`, so it opens at the target location.
#[cfg(windows)]
fn candidates(dir: &Path) -> Vec<Command> {
    use std::os::windows::process::CommandExt;
    /// Give console-subsystem fallbacks their own window (the Tauri GUI
    /// process has no console to attach to).
    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;

    let mut cmds = Vec::new();

    // Windows Terminal (default on Win11, common on Win10). The app
    // execution alias resolves through PATH; `-d` forces the start dir.
    cmds.push({
        let mut c = Command::new("wt.exe");
        c.arg("-d").arg(dir);
        c
    });

    // PowerShell 7, then Windows PowerShell 5.1 (always present). Both
    // inherit the cwd; the trailing Set-Location wins over any profile `cd`.
    for program in ["pwsh.exe", "powershell.exe"] {
        let inner = format!(
            "Set-Location -LiteralPath '{}'",
            dir.display().to_string().replace('\'', "''")
        );
        cmds.push({
            let mut c = Command::new(program);
            c.arg("-NoExit")
                .arg("-Command")
                .arg(inner)
                .current_dir(dir)
                .creation_flags(CREATE_NEW_CONSOLE);
            c
        });
    }

    cmds
}

#[cfg(target_os = "macos")]
fn candidates(dir: &Path) -> Vec<Command> {
    // Terminal.app is always present; `open` asks LaunchServices for a
    // new window at `dir` and returns immediately.
    vec![{
        let mut c = Command::new("open");
        c.arg("-a").arg("Terminal").arg(dir);
        c
    }]
}

#[cfg(all(unix, not(target_os = "macos")))]
fn candidates(dir: &Path) -> Vec<Command> {
    let mut cmds = Vec::new();
    // (program, args-before-dir); `None` args → inherits `current_dir`.
    // Flags use the long-standing spellings; `current_dir` is also set for
    // every candidate so flag-less terminals still open in place.
    let list: &[(&str, Option<&str>)] = &[
        ("x-terminal-emulator", None),
        ("gnome-terminal", Some("--working-directory")),
        ("konsole", Some("--workdir")),
        ("xfce4-terminal", Some("--working-directory")),
        ("mate-terminal", Some("--working-directory")),
        ("tilix", Some("--working-directory")),
        ("alacritty", Some("--working-directory")),
        ("kitty", Some("--directory")),
        ("wezterm", Some("start --cwd")),
        ("foot", None),
        ("qterminal", None),
    ];
    for (program, args) in list {
        let mut c = Command::new(program);
        c.current_dir(dir);
        if let Some(flag) = args {
            // Multi-word entries like "start --cwd" split on spaces.
            for part in flag.split(' ') {
                c.arg(part);
            }
            c.arg(dir);
        }
        cmds.push(c);
    }
    cmds
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates_are_non_empty_and_open_in_dir() {
        let dir = std::env::temp_dir();
        let cmds = candidates(&dir);
        assert!(!cmds.is_empty());
        for cmd in &cmds {
            let as_arg = cmd.get_args().any(|a| a == dir.as_os_str());
            let as_cwd = cmd.get_current_dir() == Some(dir.as_path());
            assert!(
                as_arg || as_cwd,
                "candidate {:?} must open in the target dir",
                cmd.get_program()
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_prefers_windows_terminal_then_powershell() {
        let cmds = candidates(Path::new("C:\\"));
        assert_eq!(cmds[0].get_program(), "wt.exe");
        assert!(cmds[1..]
            .iter()
            .any(|c| c.get_program() == "powershell.exe"));
    }

    #[test]
    fn launch_rejects_missing_dir() {
        let dir = std::env::temp_dir().join("ibexgit-nonexistent-terminal-test");
        let err = launch(&dir).expect_err("missing dir must fail");
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }
}
