//! How full the file systems the operator can fill are: on Linux the root
//! (on WSL the VM's virtual disk) and on WSL each Windows drive it mounts
//! writable, since the virtual disk lives on one of them; on Windows the
//! system drive. Which ones is read once, at start.

use super::reading::Usage;

/// A file system to report: what the operator calls it and where it is.
#[derive(Clone, Debug, PartialEq)]
pub struct Mount {
    pub name: String,
    pub path: String,
}

#[cfg(not(windows))]
pub fn mounts() -> Vec<Mount> {
    let root = Mount {
        name: "/".into(),
        path: "/".into(),
    };
    let drives = std::fs::read_to_string("/proc/mounts")
        .map(|m| drives(&m))
        .unwrap_or_default();
    std::iter::once(root).chain(drives).collect()
}

/// WSL's Windows drives in `/proc/mounts`: `9p` (or the older `drvfs`)
/// with `aname=drvfs`, mounted read-write at `/mnt/<letter>`.
pub fn drives(mounts: &str) -> Vec<Mount> {
    mounts
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let (_, path, kind, options) = (
                fields.next()?,
                fields.next()?,
                fields.next()?,
                fields.next()?,
            );
            let letter = path.strip_prefix("/mnt/")?;
            let drvfs = kind == "drvfs" || (kind == "9p" && options.contains("aname=drvfs"));
            let writable = options.split(',').any(|o| o == "rw");
            let one = letter.len() == 1 && letter.chars().all(|c| c.is_ascii_lowercase());
            (drvfs && writable && one).then(|| Mount {
                name: format!("{}:", letter.to_ascii_uppercase()),
                path: path.to_owned(),
            })
        })
        .collect()
}

/// The drive Windows runs from, like `C:`.
#[cfg(windows)]
pub fn mounts() -> Vec<Mount> {
    let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
    vec![Mount {
        path: format!("{drive}\\"),
        name: drive,
    }]
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
    #[test]
    fn wsl_reports_its_writable_windows_drives() {
        let mounts = "/dev/sdd / ext4 rw,relatime 0 0\n\
            D: /mnt/d 9p ro,noatime,aname=drvfs;path=D: 0 0\n\
            C:\\134 /mnt/c 9p rw,noatime,aname=drvfs;path=C:\\ 0 0\n\
            none /mnt/wsl tmpfs rw,relatime 0 0\n\
            E: /mnt/e drvfs rw,noatime 0 0\n";
        let names: Vec<_> = super::drives(mounts).into_iter().map(|m| m.name).collect();
        assert_eq!(names, ["C:", "E:"]);
    }

    #[cfg(unix)]
    #[test]
    fn the_root_file_system_has_a_size() {
        let root = super::usage("/").expect("statvfs on /");
        assert!(root.total > 0 && root.used <= root.total);
        assert_eq!(super::usage("/no/such/path/65"), None);
    }

    #[cfg(windows)]
    #[test]
    fn the_system_drive_has_a_size() {
        let drive = super::usage(&super::mounts()[0].path).expect("the system drive");
        assert!(drive.total > 0 && drive.used <= drive.total);
        assert_eq!(super::usage("Q:\\no\\such\\drive"), None);
    }
}
