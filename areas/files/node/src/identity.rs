//! Stable identity of the staged item across a rename on the same filesystem.
use serde::{Deserialize, Serialize};
use std::{io, path::Path};

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    volume: u64,
    file: u64,
}

#[cfg(unix)]
pub fn identity(path: &Path) -> io::Result<Identity> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        return Err(io::Error::other("staged item is a link"));
    }
    Ok(Identity {
        volume: meta.dev(),
        file: meta.ino(),
    })
}

#[cfg(windows)]
pub fn identity(path: &Path) -> io::Result<Identity> {
    use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, GetFileInformationByHandle,
    };
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(io::Error::other("staged item is a link"));
    }
    let file = std::fs::OpenOptions::new()
        .access_mode(0)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    // SAFETY: the handle is open and the output points to initialized writable memory.
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe { GetFileInformationByHandle(file.as_raw_handle().cast(), &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(Identity {
        volume: info.dwVolumeSerialNumber as u64,
        file: (info.nFileIndexHigh as u64) << 32 | info.nFileIndexLow as u64,
    })
}
