//! The Win32 calls the holder needs, behind safe functions.

pub mod conpty;
pub mod detached;
pub mod pipe;

use std::os::windows::io::{FromRawHandle, OwnedHandle};

use windows_sys::Win32::Foundation::HANDLE;

/// A NUL-terminated UTF-16 copy of `s`.
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Takes ownership of a handle a Win32 call returned.
///
/// # Safety
/// `handle` must be valid, open and owned by nobody else.
unsafe fn owned(handle: HANDLE) -> OwnedHandle {
    unsafe { OwnedHandle::from_raw_handle(handle) }
}

fn last_error() -> std::io::Error {
    std::io::Error::last_os_error()
}
