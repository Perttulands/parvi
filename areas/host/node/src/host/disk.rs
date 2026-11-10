//! How full the system's file system is: the root on Linux (on WSL that is
//! the VM's virtual disk, not a Windows drive), the system drive on Windows.

use super::reading::Usage;

/// The mount the Fleet tab reports.
#[cfg(not(windows))]
pub fn mount() -> String {
    "/".into()
}

/// The drive Windows runs from, like `C:\`.
#[cfg(windows)]
pub fn mount() -> String {
    let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
    format!("{drive}\\")
}

/// Used bytes of the file system holding `path`, out of what users can fill:
/// used plus available, leaving out the blocks reserved for root, so the
/// share is `df`'s `Use%`.
#[cfg(unix)]
pub fn usage(path: &str) -> Option<Usage> {
    let path = std::ffi::CString::new(path).ok()?;
    // SAFETY: statvfs fills `stat` and reads only the NUL-terminated path.
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(path.as_ptr(), &mut stat) } != 0 {
        return None;
    }
    let block = stat.f_frsize as u64;
    let used = (stat.f_blocks as u64).saturating_sub(stat.f_bfree as u64) * block;
    let total = used + stat.f_bavail as u64 * block;
    (total > 0).then_some(Usage { used, total })
}

/// Used bytes of the drive holding `path`, out of its size, as Explorer
/// shows it.
#[cfg(windows)]
pub fn usage(path: &str) -> Option<Usage> {
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let path: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
    let (mut available, mut total, mut free) = (0u64, 0u64, 0u64);
    // SAFETY: the path is NUL-terminated and the three counts are writable.
    if unsafe { GetDiskFreeSpaceExW(path.as_ptr(), &mut available, &mut total, &mut free) } == 0 {
        return None;
    }
    (total > 0).then(|| Usage {
        used: total.saturating_sub(free),
        total,
    })
}

#[cfg(not(any(unix, windows)))]
pub fn usage(_path: &str) -> Option<Usage> {
    None
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    #[test]
    fn the_root_file_system_has_a_size() {
        let root = super::usage(&super::mount()).expect("statvfs on /");
        assert!(root.total > 0 && root.used <= root.total);
        assert_eq!(super::usage("/no/such/path/65"), None);
    }

    #[cfg(windows)]
    #[test]
    fn the_system_drive_has_a_size() {
        let drive = super::usage(&super::mount()).expect("the system drive");
        assert!(drive.total > 0 && drive.used <= drive.total);
        assert_eq!(super::usage("Q:\\no\\such\\drive"), None);
    }
}
