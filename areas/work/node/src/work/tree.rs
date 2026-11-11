//! Holds a child process's whole tree, so that abandoning a call (a timeout,
//! or the last watcher leaving) ends everything it started, not just the
//! child: `bd` or `gh` may be a wrapper whose children hold the pipes. A process
//! group on Unix; on Windows a job object that kills what it holds when it
//! closes, with the child started suspended so nothing it starts escapes.
//! On Windows the child also gets no console window.

use std::io;
use std::process::{Output, Stdio};

use tokio::process::{Child, Command};

/// Runs `command` to its end with its output piped, holding its whole tree:
/// dropping the future before it finishes ends everything it started.
pub(super) async fn output(command: &mut Command) -> io::Result<Output> {
    prepare(command);
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let tree = Tree::adopt(&mut child)
        .map_err(|error| io::Error::new(error.kind(), format!("holding its processes: {error}")))?;
    let output = child.wait_with_output().await?;
    tree.finished();
    Ok(output)
}

/// Sets `command` up to be held once spawned.
fn prepare(command: &mut Command) {
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{CREATE_NO_WINDOW, CREATE_SUSPENDED};
        command.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
    }
}

/// A spawned child's tree. Dropping it before [`Tree::finished`] kills the
/// whole tree.
#[cfg(unix)]
struct Tree(Option<i32>);

#[cfg(unix)]
impl Tree {
    /// Holds the tree of a child spawned from a [`prepare`]d command.
    fn adopt(child: &mut Child) -> io::Result<Tree> {
        Ok(Tree(child.id().and_then(|pid| i32::try_from(pid).ok())))
    }

    /// The child exited and was reaped, so its group id may be reused:
    /// nothing is killed.
    fn finished(mut self) {
        self.0 = None;
    }
}

#[cfg(unix)]
impl Drop for Tree {
    fn drop(&mut self) {
        if let Some(pid) = self.0 {
            // SAFETY: signals the process group this call created; its
            // leader is not reaped yet, so the id is still its.
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
    }
}

#[cfg(windows)]
struct Tree {
    /// Held for its closing, which ends what the job holds.
    _job: std::os::windows::io::OwnedHandle,
}

#[cfg(windows)]
impl Tree {
    /// Puts a child spawned suspended from a [`prepare`]d command in a job,
    /// then lets it run. A child that cannot be held is killed: it is
    /// never left running unheld, nor suspended.
    fn adopt(child: &mut Child) -> io::Result<Tree> {
        // SAFETY: the handles are the child's own and the job's, which this
        // owns.
        let held = unsafe { windows::hold(child) };
        if held.is_err() {
            let _ = child.start_kill();
        }
        held.map(|job| Tree { _job: job })
    }

    /// The child exited. Closing the job still ends anything it left.
    fn finished(self) {}
}

#[cfg(windows)]
mod windows {
    use std::io;
    use std::mem::{size_of, zeroed};
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::ptr::null;

    use tokio::process::Child;
    use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };
    use windows_sys::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};

    fn raw(handle: &OwnedHandle) -> HANDLE {
        handle.as_raw_handle() as HANDLE
    }

    /// A job that kills what it holds when closed, holding the child, which
    /// then runs.
    pub(super) unsafe fn hold(child: &Child) -> io::Result<OwnedHandle> {
        let gone = || io::Error::other("the child was gone before it could be held");
        let process = child.raw_handle().ok_or_else(gone)?;
        let pid = child.id().ok_or_else(gone)?;
        unsafe {
            let job = CreateJobObjectW(null(), null());
            if job.is_null() {
                return Err(io::Error::last_os_error());
            }
            let job = OwnedHandle::from_raw_handle(job as _);
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                raw(&job),
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ) == 0
                || AssignProcessToJobObject(raw(&job), process as HANDLE) == 0
            {
                return Err(io::Error::last_os_error());
            }
            resume(pid)?;
            Ok(job)
        }
    }

    /// Resumes the threads of a process started suspended: its one thread.
    unsafe fn resume(pid: u32) -> io::Result<()> {
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
            if snapshot == INVALID_HANDLE_VALUE {
                return Err(io::Error::last_os_error());
            }
            let snapshot = OwnedHandle::from_raw_handle(snapshot as _);
            let mut entry: THREADENTRY32 = zeroed();
            entry.dwSize = size_of::<THREADENTRY32>() as u32;
            let mut resumed = false;
            let mut more = Thread32First(raw(&snapshot), &mut entry) != 0;
            while more {
                if entry.th32OwnerProcessID == pid {
                    let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                    if !thread.is_null() {
                        let thread = OwnedHandle::from_raw_handle(thread as _);
                        resumed |= ResumeThread(raw(&thread)) != u32::MAX;
                    }
                }
                more = Thread32Next(raw(&snapshot), &mut entry) != 0;
            }
            if resumed {
                Ok(())
            } else {
                Err(io::Error::other("could not resume the child"))
            }
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod linux_tests {
    use std::time::{Duration, Instant};

    use super::*;

    /// Running and not a zombie.
    fn alive(pid: &str) -> bool {
        std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
            stat.rsplit(") ")
                .next()
                .is_some_and(|state| !state.starts_with('Z'))
        })
    }

    #[tokio::test]
    async fn abandoning_a_call_ends_what_its_child_started() {
        let dir = std::env::temp_dir().join(format!("parvi-213-tree-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pidfile = dir.join("pid");
        let mut command = Command::new("sh");
        command
            .args([
                "-c",
                r#"sleep 60 & echo $! > "$1.tmp"; mv "$1.tmp" "$1"; wait"#,
                "sh",
            ])
            .arg(&pidfile);
        let call = tokio::spawn(async move { output(&mut command).await });
        let deadline = Instant::now() + Duration::from_secs(10);
        let grandchild = loop {
            if let Ok(pid) = std::fs::read_to_string(&pidfile) {
                break pid.trim().to_owned();
            }
            assert!(Instant::now() < deadline, "the child started nothing");
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        assert!(alive(&grandchild));
        call.abort();
        assert!(call.await.unwrap_err().is_cancelled());
        let deadline = Instant::now() + Duration::from_secs(5);
        while alive(&grandchild) {
            assert!(
                Instant::now() < deadline,
                "the grandchild outlived the call"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(all(test, windows))]
mod tests {
    use std::mem::{size_of, zeroed};
    use std::time::{Duration, Instant};

    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };

    use super::*;

    /// The processes whose parent is `pid`.
    fn children_of(pid: u32) -> Vec<u32> {
        let mut found = Vec::new();
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            let mut entry: PROCESSENTRY32W = zeroed();
            entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
            let mut more = Process32FirstW(snapshot, &mut entry) != 0;
            while more {
                if entry.th32ParentProcessID == pid {
                    found.push(entry.th32ProcessID);
                }
                more = Process32NextW(snapshot, &mut entry) != 0;
            }
            CloseHandle(snapshot);
        }
        found
    }

    #[tokio::test]
    async fn abandoning_a_call_ends_what_its_child_started() {
        let mut command = Command::new("cmd.exe");
        command
            .args(["/c", "ping -n 60 127.0.0.1 >NUL"])
            .stdout(std::process::Stdio::null())
            .kill_on_drop(true);
        prepare(&mut command);
        let mut child = command.spawn().unwrap();
        let tree = Tree::adopt(&mut child).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let grandchild = loop {
            if let Some(&pid) = children_of(child.id().unwrap()).first() {
                break pid;
            }
            assert!(Instant::now() < deadline, "the child started nothing");
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        let watch = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, grandchild) };
        assert!(!watch.is_null());
        drop(tree);
        drop(child);
        assert_eq!(unsafe { WaitForSingleObject(watch, 5000) }, WAIT_OBJECT_0);
        unsafe { CloseHandle(watch) };
    }
}
