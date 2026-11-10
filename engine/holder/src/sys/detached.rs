//! Starting the holder so that nothing ties it to its starter.

use std::mem::{size_of, zeroed};
use std::ptr::null;

use anyhow::{Result, bail};
use windows_sys::Win32::System::Threading::{
    CREATE_BREAKAWAY_FROM_JOB, CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DETACHED_PROCESS,
    PROCESS_INFORMATION, STARTF_USESHOWWINDOW, STARTUPINFOW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;

use super::{last_error, owned, wide};

/// Starts `command_line` hidden, with no console, no inherited handles and in
/// `cwd`, out of the starter's job where Windows allows. Returns its pid.
///
/// Rust's `Command` passes `bInheritHandles = TRUE`, so a child started with
/// it keeps the starter's inheritable pipes open, and whoever reads the
/// starter's output (PowerShell, WSL interop) waits for the holder to exit.
/// Not `CREATE_NEW_PROCESS_GROUP` either: it disables Ctrl+C in the new
/// process, and every shell the holder starts would inherit that.
pub fn start(command_line: &str, cwd: Option<&str>) -> Result<u32> {
    let flags = DETACHED_PROCESS | CREATE_UNICODE_ENVIRONMENT;
    create(command_line, cwd, flags | CREATE_BREAKAWAY_FROM_JOB)
        .or_else(|_| create(command_line, cwd, flags))
}

fn create(command_line: &str, cwd: Option<&str>, flags: u32) -> Result<u32> {
    let mut si: STARTUPINFOW = unsafe { zeroed() };
    si.cb = size_of::<STARTUPINFOW>() as u32;
    si.dwFlags = STARTF_USESHOWWINDOW;
    si.wShowWindow = SW_HIDE as u16;
    let mut pi: PROCESS_INFORMATION = unsafe { zeroed() };
    let mut command = wide(command_line);
    let cwd = cwd.map(wide);
    let ok = unsafe {
        CreateProcessW(
            null(),
            command.as_mut_ptr(),
            null(),
            null(),
            0,
            flags,
            null(),
            cwd.as_ref().map_or(null(), |w| w.as_ptr()),
            &si,
            &mut pi,
        )
    };
    if ok == 0 {
        bail!("CreateProcessW {command_line:?}: {}", last_error());
    }
    drop(unsafe { (owned(pi.hProcess), owned(pi.hThread)) });
    Ok(pi.dwProcessId)
}
