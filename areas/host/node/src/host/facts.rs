//! What the host is, read once at start: names that do not change while the
//! node runs.

pub use parvi_host_contract::Facts;

#[cfg(not(windows))]
pub fn read() -> Facts {
    use super::procfs;
    let read = |path: &str| std::fs::read_to_string(path).ok();
    let kernel = read("/proc/sys/kernel/osrelease").map(|k| k.trim().to_owned());
    Facts {
        os: read("/etc/os-release")
            .as_deref()
            .and_then(procfs::parse_os_name),
        kernel,
        cpu: read("/proc/cpuinfo")
            .as_deref()
            .and_then(procfs::parse_cpu_model),
        threads: threads(),
        user: user("USER"),
    }
}

#[cfg(windows)]
pub fn read() -> Facts {
    use super::windows;
    Facts {
        os: windows::os_name(),
        kernel: windows::kernel(),
        cpu: windows::cpu_model(),
        threads: threads(),
        user: user("USERNAME"),
    }
}

fn threads() -> usize {
    std::thread::available_parallelism().map_or(0, usize::from)
}

fn user(variable: &str) -> Option<String> {
    std::env::var(variable).ok().filter(|user| !user.is_empty())
}
