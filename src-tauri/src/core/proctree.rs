//! Process-tree lifecycle primitives for the `GitProcessRunner`.
//!
//! Git 不孤立：hooks、credential helper、`ssh`/`askpass` 都会派生子进程。
//! cancel/timeout 时只 kill 直接子进程会把孙进程变成孤儿（例如仍握着
//! credential helper 的会话）。因此终止必须作用于**整棵进程树**：
//!
//! - **Windows**：子进程以 `CREATE_SUSPENDED` 启动 → 建立带
//!   `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` 的 Job Object → `AssignProcessToJobObject`
//!   → 恢复主线程。kill = `TerminateJobObject`（一击必杀整棵树）；
//!   即使 panic 路径漏掉显式 kill，Job 句柄关闭时内核仍会兜底清树。
//!   挂起启动保证 git 没来得及派生任何子进程就已被收编，杜绝逃逸窗口。
//! - **Unix**：子进程 `process_group(0)` 自成进程组，kill = `killpg(SIGKILL)`。
//!
//! 非 Windows/Unix 平台降级为只 kill 直接子进程。

use std::io;
use tokio::process::{Child, Command};

/// A spawned process together with a mechanism to kill its whole tree.
pub struct TreeChild {
    child: Child,
    kill: KillKind,
}

enum KillKind {
    #[cfg(windows)]
    Job(JobHandle),
    #[cfg(unix)]
    ProcessGroup(i32),
    /// No tree mechanism available (unknown platform, or the Windows job
    /// assignment was refused) — fall back to killing the direct child only.
    #[cfg(not(unix))]
    Direct,
}

impl TreeChild {
    /// Spawn `cmd` as a tree-killable child.
    pub async fn spawn(cmd: &mut Command) -> io::Result<Self> {
        #[cfg(windows)]
        {
            Self::spawn_windows(cmd).await
        }
        #[cfg(unix)]
        {
            Self::spawn_unix(cmd)
        }
        #[cfg(not(any(windows, unix)))]
        {
            let child = cmd.spawn()?;
            Ok(Self {
                child,
                kill: KillKind::Direct,
            })
        }
    }

    /// PID of the direct child, if it is still running.
    pub fn id(&self) -> Option<u32> {
        self.child.id()
    }

    /// Take the child's stdin handle (once) — used for `StdinMode::Feed`.
    pub fn take_stdin(&mut self) -> Option<tokio::process::ChildStdin> {
        self.child.stdin.take()
    }

    /// Take the child's stderr handle (once) — used by the P7 streaming
    /// progress path (`clone --progress`).
    pub fn take_stderr(&mut self) -> Option<tokio::process::ChildStderr> {
        self.child.stderr.take()
    }

    /// Fire the tree kill. Reap with [`TreeChild::wait`] afterwards.
    pub fn kill_tree(&mut self) {
        match &self.kill {
            #[cfg(windows)]
            KillKind::Job(job) => unsafe {
                // Terminates every process in the job; the direct child
                // included. `start_kill` below is a belt-and-braces no-op
                // when the assignment succeeded.
                windows_sys::Win32::System::JobObjects::TerminateJobObject(job.0, 0xC1);
                let _ = self.child.start_kill();
            },
            #[cfg(unix)]
            KillKind::ProcessGroup(pgid) => {
                // ESRCH is fine — the group may already be gone.
                unsafe {
                    libc::killpg(*pgid, libc::SIGKILL);
                }
                let _ = self.child.start_kill();
            }
            #[cfg(not(unix))]
            KillKind::Direct => {
                let _ = self.child.start_kill();
            }
        }
    }

    /// Reap the direct child after [`TreeChild::kill_tree`] or normal exit.
    pub async fn wait(&mut self) -> io::Result<std::process::ExitStatus> {
        self.child.wait().await
    }

    /// Collect stdout/stderr to completion (stdin must already be taken or null).
    ///
    /// Unlike [`tokio::process::Child::wait_with_output`] (which consumes the
    /// child), this takes `&mut self` so the caller can still kill the tree
    /// when a concurrent timeout/cancel future wins the race.
    pub async fn wait_with_output(&mut self) -> io::Result<std::process::Output> {
        use tokio::io::AsyncReadExt;

        async fn drain<R>(reader: Option<R>) -> io::Result<Vec<u8>>
        where
            R: tokio::io::AsyncRead + Unpin,
        {
            match reader {
                Some(mut r) => {
                    let mut buf = Vec::new();
                    r.read_to_end(&mut buf).await?;
                    Ok(buf)
                }
                None => Ok(Vec::new()),
            }
        }

        let stdout = drain(self.child.stdout.take());
        let stderr = drain(self.child.stderr.take());
        let (stdout, stderr, status) = tokio::try_join!(stdout, stderr, self.child.wait())?;
        Ok(std::process::Output {
            status,
            stdout,
            stderr,
        })
    }

    #[cfg(windows)]
    async fn spawn_windows(cmd: &mut Command) -> io::Result<Self> {
        const CREATE_SUSPENDED: u32 = 0x0000_0004;

        // The job is created up-front so a spawn failure never leaves a
        // half-initialized kill mechanism behind.
        let job = JobHandle::create()?;

        cmd.creation_flags(CREATE_SUSPENDED);
        let mut child = cmd.spawn()?;
        let Some(pid) = child.id() else {
            let _ = child.start_kill();
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "spawned child has no pid",
            ));
        };

        // Assign before first instruction runs (process is suspended).
        let assigned = match child.raw_handle() {
            Some(handle) => unsafe {
                windows_sys::Win32::System::JobObjects::AssignProcessToJobObject(job.0, handle) != 0
            },
            None => false,
        };

        if !assigned {
            tracing::warn!(
                pid,
                "AssignProcessToJobObject failed ({}); \
                 falling back to direct-child kill",
                io::Error::last_os_error()
            );
        }

        // A suspended process that is never resumed would hang forever:
        // any resume failure must kill the child, never return it.
        if let Err(e) = resume_initial_thread(pid) {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return Err(e);
        }

        let kill = if assigned {
            KillKind::Job(job)
        } else {
            KillKind::Direct
        };
        Ok(Self { child, kill })
    }

    #[cfg(unix)]
    fn spawn_unix(cmd: &mut Command) -> io::Result<Self> {
        // New process group in the same session; killpg reaches every
        // descendant that did not explicitly setsid away.
        cmd.process_group(0);
        let child = cmd.spawn()?;
        let pgid = child.id().unwrap_or(0) as i32;
        Ok(Self {
            child,
            kill: KillKind::ProcessGroup(pgid),
        })
    }
}

/// Job Object handle with `KILL_ON_JOB_CLOSE`: dropping the last handle to a
/// non-empty job makes the kernel kill its processes — an orphan safety net.
///
/// Kernel handles are context-independent and not thread-affine, so moving the
/// owning wrapper across threads is sound (same reasoning as
/// `std::os::windows::io::OwnedHandle`, which is `Send + Sync`).
#[cfg(windows)]
struct JobHandle(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
// SAFETY: the wrapper exclusively owns the handle; see doc comment above.
unsafe impl Send for JobHandle {}

#[cfg(windows)]
impl JobHandle {
    fn create() -> io::Result<Self> {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::JobObjects::{
            CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };

        unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok == 0 {
                let err = io::Error::last_os_error();
                CloseHandle(handle);
                return Err(err);
            }
            Ok(JobHandle(handle))
        }
    }
}

#[cfg(windows)]
impl Drop for JobHandle {
    fn drop(&mut self) {
        unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0) };
    }
}

/// Resume the (single) initial thread of a `CREATE_SUSPENDED` process.
#[cfg(windows)]
fn resume_initial_thread(pid: u32) -> io::Result<()> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
    };
    use windows_sys::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};

    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }

        let mut entry: THREADENTRY32 = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
        let mut resumed = false;

        if Thread32First(snapshot, &mut entry) != 0 {
            loop {
                if entry.th32OwnerProcessID == pid {
                    let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                    if !thread.is_null() {
                        let previous = ResumeThread(thread);
                        CloseHandle(thread);
                        if previous != u32::MAX {
                            resumed = true;
                        }
                        break;
                    }
                }
                if Thread32Next(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }

        CloseHandle(snapshot);
        if resumed {
            Ok(())
        } else {
            Err(io::Error::other(format!(
                "could not resume initial thread of pid {pid}"
            )))
        }
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;
    use std::time::Duration;

    /// Direct + grandchild PIDs of `parent` via a Toolhelp process snapshot.
    /// `CreateToolhelp32Snapshot` can fail transiently under load — retry.
    fn child_pids(parent: u32) -> Vec<u32> {
        for attempt in 0..3 {
            match try_child_pids(parent) {
                Some(pids) => return pids,
                None => std::thread::sleep(Duration::from_millis(50 * (attempt + 1))),
            }
        }
        panic!("CreateToolhelp32Snapshot failed repeatedly");
    }

    fn try_child_pids(parent: u32) -> Option<Vec<u32>> {
        use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
            TH32CS_SNAPPROCESS,
        };
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snapshot == INVALID_HANDLE_VALUE {
                return None;
            }
            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            let mut pids = Vec::new();
            if Process32FirstW(snapshot, &mut entry) != 0 {
                loop {
                    if entry.th32ParentProcessID == parent {
                        pids.push(entry.th32ProcessID);
                    }
                    if Process32NextW(snapshot, &mut entry) == 0 {
                        break;
                    }
                }
            }
            CloseHandle(snapshot);
            Some(pids)
        }
    }

    /// A pid is dead when the kernel object no longer signals WAIT_TIMEOUT.
    fn pid_alive(pid: u32) -> bool {
        use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
        use windows_sys::Win32::System::Threading::{
            OpenProcess, WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION,
            PROCESS_SYNCHRONIZE,
        };
        unsafe {
            let handle = OpenProcess(
                PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
                0,
                pid,
            );
            if handle.is_null() {
                return false; // gone (or never existed)
            }
            let state = WaitForSingleObject(handle, 0);
            CloseHandle(handle);
            state != WAIT_OBJECT_0
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn kill_tree_terminates_descendants() {
        // test → cmd → ping: the job must reach the grandchild.
        let mut cmd = Command::new("cmd");
        cmd.arg("/c")
            .arg("ping -n 30 127.0.0.1")
            .stdin(std::process::Stdio::null());
        let mut tree = TreeChild::spawn(&mut cmd).await.expect("spawn cmd");
        let pid = tree.id().expect("pid");
        assert!(job_assigned(&tree), "child must be assigned to a job");

        // Give cmd a moment to actually spawn ping.
        let mut ping = Vec::new();
        for _ in 0..50 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            ping = child_pids(pid);
            if !ping.is_empty() {
                break;
            }
        }
        assert!(
            !ping.is_empty(),
            "expected cmd to have spawned ping as a child"
        );
        assert!(ping.iter().all(|&p| pid_alive(p)));

        tree.kill_tree();
        let status = tree.wait().await.expect("wait cmd");
        assert!(!status.success(), "killed cmd must not exit cleanly");

        for p in ping {
            // Termination signaled by TerminateJobObject is asynchronous on
            // Windows — poll until the pid is reaped (bounded, generous).
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            loop {
                if !pid_alive(p) {
                    break;
                }
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "grandchild pid {p} must be dead after TerminateJobObject"
                );
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }

    fn job_assigned(tree: &TreeChild) -> bool {
        matches!(tree.kill, KillKind::Job(_))
    }
}

#[cfg(all(test, unix))]
mod unix_tests {
    use super::*;
    use std::time::Duration;

    /// `kill(pid, 0)`: ESRCH ⇒ no such process (alive = otherwise).
    fn pid_alive(pid: i32) -> bool {
        let rc = unsafe { libc::kill(pid, 0) };
        !(rc == -1 && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH))
    }

    fn child_pids(parent: u32) -> Vec<i32> {
        // `ps -axo pid=,ppid=` works on Linux and macOS alike.
        let out = std::process::Command::new("ps")
            .args(["-axo", "pid=,ppid="])
            .output()
            .expect("ps available");
        let text = String::from_utf8_lossy(&out.stdout);
        text.lines()
            .filter_map(|line| {
                let mut it = line.split_whitespace();
                let pid: i32 = it.next()?.parse().ok()?;
                let ppid: u32 = it.next()?.parse().ok()?;
                (ppid == parent).then_some(pid)
            })
            .collect()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn kill_tree_terminates_descendants() {
        // sh stays alive (background job + wait), two sleeps are its children.
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg("sleep 30 & sleep 30; wait")
            .stdin(std::process::Stdio::null());
        let mut tree = TreeChild::spawn(&mut cmd).await.expect("spawn sh");
        let pid = tree.id().expect("pid");
        assert!(matches!(tree.kill, KillKind::ProcessGroup(_)));

        let mut children = Vec::new();
        for _ in 0..50 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            children = child_pids(pid);
            if !children.is_empty() {
                break;
            }
        }
        assert!(!children.is_empty(), "expected sleep children under sh pid");
        assert!(children.iter().all(|&p| pid_alive(p)));

        tree.kill_tree();
        let status = tree.wait().await.expect("wait sh");
        assert!(!status.success(), "SIGKILLed sh must not exit cleanly");

        // SIGKILL 送达是即时的，但收尸不是：孙进程被 init/launchd 回收前
        // 是 zombie，`kill(pid, 0)` 仍会成功（系统高负载时窗口明显变大）。
        // 轮询等它们全部消失，而不是在 race window 里硬断言。
        for _ in 0..100 {
            if children.iter().all(|&p| !pid_alive(p)) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        for p in children {
            assert!(
                !pid_alive(p),
                "child pid {p} must be dead after killpg(SIGKILL)"
            );
        }
    }
}
