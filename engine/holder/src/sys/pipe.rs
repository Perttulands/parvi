//! The holder's named pipe: one per user, open to that user alone.

use std::ffi::c_void;
use std::io;
use std::mem::size_of;
use std::ptr::null_mut;
use std::time::Duration;

use anyhow::{Result, bail};
use tokio::net::windows::named_pipe::{
    ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions,
};
use windows_sys::Win32::Foundation::{ERROR_PIPE_BUSY, HANDLE, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
    TokenUser,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

use super::{last_error, owned, wide};

/// Owner and SYSTEM only, protected from inherited entries.
const OWNER_ONLY: &str = "D:P(A;;GA;;;OW)(A;;GA;;;SY)";

/// `\\.\pipe\parvi-holder-<suffix>`.
pub fn path(suffix: &str) -> String {
    format!(r"\\.\pipe\parvi-holder-{suffix}")
}

/// This user's SID as a string (`S-1-5-21-…`): unlike the user name, it
/// cannot collide between a local and a domain account.
pub fn user_sid() -> Result<String> {
    unsafe {
        let mut token: HANDLE = null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            bail!("OpenProcessToken: {}", last_error());
        }
        let token = owned(token);
        let raw = std::os::windows::io::AsRawHandle::as_raw_handle(&token) as HANDLE;
        let mut size = 0u32;
        GetTokenInformation(raw, TokenUser, null_mut(), 0, &mut size);
        // u64 storage keeps TOKEN_USER aligned.
        let mut buf = vec![0u64; (size as usize).div_ceil(8)];
        if GetTokenInformation(raw, TokenUser, buf.as_mut_ptr().cast(), size, &mut size) == 0 {
            bail!("GetTokenInformation: {}", last_error());
        }
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut text: *mut u16 = null_mut();
        if ConvertSidToStringSidW(user.User.Sid, &mut text) == 0 {
            bail!("ConvertSidToStringSidW: {}", last_error());
        }
        let len = (0..).take_while(|&i| *text.add(i) != 0).count();
        let sid = String::from_utf16_lossy(std::slice::from_raw_parts(text, len));
        LocalFree(text.cast());
        Ok(sid)
    }
}

/// Creates an instance of the pipe that only this user may open, refusing
/// remote clients. `first` fails if any instance exists already: that is
/// what keeps one holder per pipe.
pub fn listen(path: &str, first: bool) -> io::Result<NamedPipeServer> {
    let sddl = wide(OWNER_ONLY);
    let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            null_mut(),
        )
    } == 0
    {
        return Err(last_error());
    }
    let mut attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    let mut options = ServerOptions::new();
    options
        .first_pipe_instance(first)
        .reject_remote_clients(true);
    let server = unsafe {
        options.create_with_security_attributes_raw(path, (&raw mut attributes).cast::<c_void>())
    };
    unsafe { LocalFree(descriptor) };
    server
}

/// Opens the pipe, waiting out the moment between one client taking the
/// free instance and the holder offering the next.
pub async fn open(path: &str) -> io::Result<NamedPipeClient> {
    for _ in 0..250 {
        match ClientOptions::new().open(path) {
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) => {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            opened => return opened,
        }
    }
    Err(io::Error::other("the holder's pipe stayed busy"))
}
