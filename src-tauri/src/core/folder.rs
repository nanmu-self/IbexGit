//! OS-level "open a directory in the system file manager" (P3 工具栏：在文件
//! 管理器中打开). Not a git operation — app-layer OS integration like
//! `terminal`.
//!
//! Deliberately NOT `tauri-plugin-opener::open_path`: on Windows (with its
//! `shellexecute-on-windows` feature) a directory is mapped to
//! `SHOpenFolderAndSelectItems`, which reveals/selects the folder in its
//! PARENT window and may not even bring that window forward — reveal
//! semantics, not "enter". Here every platform navigates INTO the folder.

use std::io;
use std::path::Path;
use std::process::Command;

/// Open the system file manager showing the contents of `dir`.
pub fn open_in_file_manager(dir: &Path) -> io::Result<()> {
    if !dir.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("not a directory: {}", dir.display()),
        ));
    }
    spawn(command(dir))
}

/// Platform command that navigates INTO `dir`. Built separately from
/// spawning so tests can inspect it without opening windows.
#[cfg(windows)]
fn command(dir: &Path) -> Command {
    // `explorer.exe <dir>` opens the folder itself. (ShellExecute "open"
    // would delegate to the same SHOpenFolderAndSelectItems reveal path.)
    let mut cmd = Command::new("explorer.exe");
    cmd.arg(dir);
    cmd
}

#[cfg(target_os = "macos")]
fn command(dir: &Path) -> Command {
    // `open <dir>` shows the folder contents in Finder.
    let mut cmd = Command::new("open");
    cmd.arg(dir);
    cmd
}

#[cfg(all(unix, not(target_os = "macos")))]
fn command(dir: &Path) -> Command {
    // xdg-open on a directory opens it in the default file manager.
    let mut cmd = Command::new("xdg-open");
    cmd.arg(dir);
    cmd
}

#[cfg(windows)]
fn spawn(mut cmd: Command) -> io::Result<()> {
    // Explorer is always on PATH; spawn returns immediately, the window
    // outlives us, and its exit code is meaningless — fire and forget.
    cmd.spawn().map(|_| ())
}

#[cfg(not(windows))]
fn spawn(mut cmd: Command) -> io::Result<()> {
    cmd.spawn().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_targets_dir_argument() {
        let dir = std::env::temp_dir();
        let cmd = command(&dir);
        assert!(
            cmd.get_args().any(|a| a == dir.as_os_str()),
            "the directory must be passed so the FM navigates into it"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_uses_explorer() {
        assert_eq!(command(Path::new("C:\\")).get_program(), "explorer.exe");
    }

    #[test]
    fn rejects_missing_dir() {
        let dir = std::env::temp_dir().join("ibexgit-nonexistent-folder-test");
        let err = open_in_file_manager(&dir).expect_err("missing dir must fail");
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }
}
