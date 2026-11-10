//! One pseudo console, the process in it, and the job that holds the
//! process's tree.

use std::ffi::c_void;
use std::fs::File;
use std::mem::{size_of, zeroed};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, OwnedHandle};
use std::ptr::{null, null_mut};

use anyhow::{Result, bail};
use windows_sys::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
use windows_sys::Win32::System::Console::{
    COORD, ClosePseudoConsole, CreatePseudoConsole, HPCON, ResizePseudoConsole,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList,
    EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess, INFINITE, InitializeProcThreadAttributeList,
    LPPROC_THREAD_ATTRIBUTE_LIST, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, PROCESS_INFORMATION,
    ResumeThread, STARTF_USESTDHANDLES, STARTUPINFOEXW, TerminateProcess,
    UpdateProcThreadAttribute, WaitForSingleObject,
};

use super::{last_error, owned, wide};

/// Not in windows-sys: ConPTY forwards the program's VT as written instead
/// of rendering it again (Windows 11 22H2 and later).
const PSEUDOCONSOLE_PASSTHROUGH_MODE: u32 = 0x8;

/// A started pseudo console session.
pub struct Spawned {
    pub console: Console,
    /// The console's input: what the program reads.
    pub input: File,
    /// The console's output: what the program writes, as VT.
    pub output: File,
    pub process: Process,
    pub pid: u32,
    /// Whether the console forwards VT as written. Older Windows lacks it.
    pub passthrough: bool,
}

/// The pseudo console itself.
pub struct Console(HPCON);

// SAFETY: an HPCON is a handle; ConPTY's functions may be called from any
// thread.
unsafe impl Send for Console {}
unsafe impl Sync for Console {}

impl Console {
    pub fn resize(&self, cols: u16, rows: u16) {
        unsafe {
            ResizePseudoConsole(self.0, coord(cols, rows));
        }
    }

    /// Closes the console: its clients get CTRL_CLOSE_EVENT. Blocks until the
    /// output pipe is drained, so call it with no lock the reader needs.
    pub fn close(self) {
        unsafe { ClosePseudoConsole(self.0) }
    }
}

/// The program in a console, and the job holding it and its children.
pub struct Process {
    handle: OwnedHandle,
    job: Option<OwnedHandle>,
}

impl Process {
    /// Blocks until the program ends; returns its exit code.
    pub fn wait(&self) -> u32 {
        let handle = self.handle.as_raw_handle() as HANDLE;
        unsafe {
            if WaitForSingleObject(handle, INFINITE) != WAIT_OBJECT_0 {
                return u32::MAX;
            }
            let mut code = 0u32;
            GetExitCodeProcess(handle, &mut code);
            code
        }
    }

    /// Ends the program and everything it started, as closing a console
    /// window would.
    pub fn kill(&self) {
        unsafe {
            match &self.job {
                Some(job) => TerminateJobObject(job.as_raw_handle() as HANDLE, 1),
                None => TerminateProcess(self.handle.as_raw_handle() as HANDLE, 1),
            };
        }
    }
}

fn coord(cols: u16, rows: u16) -> COORD {
    COORD {
        X: cols as i16,
        Y: rows as i16,
    }
}

/// Starts `command_line` in a new pseudo console of this size.
pub fn spawn(
    command_line: &str,
    cwd: Option<&str>,
    env: &[(String, String)],
    cols: u16,
    rows: u16,
) -> Result<Spawned> {
    let (input_read, input) = pipe()?;
    let (output, output_write) = pipe()?;
    let (console, passthrough) = create_console(&input_read, &output_write, cols, rows)?;
    // The console holds its own copies of its ends.
    drop((input_read, output_write));
    let (process, pid) = match start_in(&console, command_line, cwd, env) {
        Ok(started) => started,
        Err(e) => {
            console.close();
            return Err(e);
        }
    };
    Ok(Spawned {
        console,
        input: File::from(input),
        output: File::from(output),
        process,
        pid,
        passthrough,
    })
}

/// An anonymous pipe: (read end, write end), neither inheritable.
fn pipe() -> Result<(OwnedHandle, OwnedHandle)> {
    let (mut read, mut write): (HANDLE, HANDLE) = (null_mut(), null_mut());
    if unsafe { CreatePipe(&mut read, &mut write, null(), 0) } == 0 {
        bail!("CreatePipe: {}", last_error());
    }
    Ok(unsafe { (owned(read), owned(write)) })
}

/// A console in passthrough mode where Windows has it, else a plain one.
fn create_console(
    input: &OwnedHandle,
    output: &OwnedHandle,
    cols: u16,
    rows: u16,
) -> Result<(Console, bool)> {
    let (input, output) = (
        input.as_raw_handle() as HANDLE,
        output.as_raw_handle() as HANDLE,
    );
    for (flags, passthrough) in [(PSEUDOCONSOLE_PASSTHROUGH_MODE, true), (0, false)] {
        let mut hpc: HPCON = unsafe { zeroed() };
        let hr = unsafe { CreatePseudoConsole(coord(cols, rows), input, output, flags, &mut hpc) };
        if hr == 0 {
            return Ok((Console(hpc), passthrough));
        }
        if !passthrough {
            bail!("CreatePseudoConsole: 0x{hr:08x}");
        }
    }
    unreachable!("the plain console either starts or fails")
}

/// Starts the program attached to `console`, suspended until it is in its
/// own job, so nothing it starts escapes a kill.
fn start_in(
    console: &Console,
    command_line: &str,
    cwd: Option<&str>,
    env: &[(String, String)],
) -> Result<(Process, u32)> {
    let mut list_size = 0usize;
    unsafe { InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut list_size) };
    let mut list_buf = vec![0u8; list_size];
    let list = list_buf.as_mut_ptr() as LPPROC_THREAD_ATTRIBUTE_LIST;
    unsafe {
        if InitializeProcThreadAttributeList(list, 1, 0, &mut list_size) == 0 {
            bail!("InitializeProcThreadAttributeList: {}", last_error());
        }
        if UpdateProcThreadAttribute(
            list,
            0,
            PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
            console.0 as *const c_void,
            size_of::<HPCON>(),
            null_mut(),
            null(),
        ) == 0
        {
            DeleteProcThreadAttributeList(list);
            bail!("UpdateProcThreadAttribute: {}", last_error());
        }
    }
    let mut si: STARTUPINFOEXW = unsafe { zeroed() };
    si.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    // Null std handles, or a program started by a process with redirected
    // stdio inherits those instead of the pseudo console.
    si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    si.lpAttributeList = list;
    let mut pi: PROCESS_INFORMATION = unsafe { zeroed() };
    let mut command = wide(command_line);
    let cwd = cwd.map(wide);
    // CreateProcess needs a sorted, double-NUL-terminated UTF-16 block.
    let environment = if env.is_empty() {
        None
    } else {
        let mut vars: std::collections::BTreeMap<String, (std::ffi::OsString, std::ffi::OsString)> =
            std::env::vars_os()
                .map(|(k, v)| (k.to_string_lossy().to_uppercase(), (k, v)))
                .collect();
        for (key, value) in env {
            vars.insert(key.to_uppercase(), (key.into(), value.into()));
        }
        let mut block = Vec::new();
        for (_, (key, value)) in vars {
            block.extend(key.encode_wide());
            block.push(b'=' as u16);
            block.extend(value.encode_wide());
            block.push(0);
        }
        block.push(0);
        Some(block)
    };
    let ok = unsafe {
        CreateProcessW(
            null(),
            command.as_mut_ptr(),
            null(),
            null(),
            0,
            EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT | CREATE_SUSPENDED,
            environment
                .as_ref()
                .map_or(null(), |block| block.as_ptr() as *const c_void),
            cwd.as_ref().map_or(null(), |w| w.as_ptr()),
            &si.StartupInfo,
            &mut pi,
        )
    };
    unsafe { DeleteProcThreadAttributeList(list) };
    if ok == 0 {
        bail!("CreateProcessW {command_line:?}: {}", last_error());
    }
    let (handle, thread) = unsafe { (owned(pi.hProcess), owned(pi.hThread)) };
    let job = job_for(&handle);
    unsafe { ResumeThread(thread.as_raw_handle() as HANDLE) };
    Ok((Process { handle, job }, pi.dwProcessId))
}

/// A job holding the process. Closing it, which the holder's end does, ends
/// everything in it, so nothing of a session outlives its holder.
/// `None` where Windows refuses one (a parent job that forbids nesting); a
/// kill then ends the program alone.
fn job_for(process: &OwnedHandle) -> Option<OwnedHandle> {
    unsafe {
        let job = CreateJobObjectW(null(), null());
        if job.is_null() {
            return None;
        }
        let job = owned(job);
        let raw = job.as_raw_handle() as HANDLE;
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(
            raw,
            JobObjectExtendedLimitInformation,
            (&raw const limits).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        (AssignProcessToJobObject(raw, process.as_raw_handle() as HANDLE) != 0).then_some(job)
    }
}

/// Ctrl+C handling is inherited: makes sure the programs the holder starts
/// get it, whatever its starter set.
pub fn let_children_take_ctrl_c() {
    unsafe {
        windows_sys::Win32::System::Console::SetConsoleCtrlHandler(None, 0);
    }
}
