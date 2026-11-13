//! Readings from Linux's `/proc`: CPU time, memory, swap, uptime, the
//! processes, OOM kills and what the machine is. Parsing is apart from reading, so the parsers are
//! tested on fixed text. Ported from CHROTE's `system.go`, which counts
//! iowait as idle and falls back from `MemAvailable` on old kernels.

use super::cpu::CpuTimes;
use super::heavy::Sample;
use super::reading::Usage;

fn read(path: &str) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

pub fn cpu_times() -> Option<CpuTimes> {
    parse_cpu(&read("/proc/stat")?)
}

/// The aggregate `cpu ` line of `/proc/stat`; idle includes iowait.
pub fn parse_cpu(stat: &str) -> Option<CpuTimes> {
    let line = stat.lines().find(|line| line.starts_with("cpu "))?;
    let fields: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    let idle = fields.get(3)? + fields.get(4).unwrap_or(&0);
    // guest and guest_nice (fields 9 and 10) are already counted in user.
    let total = fields.iter().take(8).sum();
    Some(CpuTimes { total, idle })
}

pub fn memory() -> Option<Usage> {
    parse_memory(&read("/proc/meminfo")?)
}

pub fn swap() -> Option<Usage> {
    parse_swap(&read("/proc/meminfo")?)
}

pub fn swap_pages() -> Option<(u64, u64)> {
    parse_swap_pages(&read("/proc/vmstat")?)
}

fn parse_swap_pages(vmstat: &str) -> Option<(u64, u64)> {
    Some((
        vmstat_field(vmstat, "pswpin")?,
        vmstat_field(vmstat, "pswpout")?,
    ))
}

/// How often the kernel has killed a process for memory since boot.
pub fn oom_kills() -> Option<u64> {
    vmstat_field(&read("/proc/vmstat")?, "oom_kill")
}

fn vmstat_field(vmstat: &str, name: &str) -> Option<u64> {
    vmstat.lines().find_map(|line| {
        let (key, value) = line.split_once(char::is_whitespace)?;
        (key == name).then(|| value.trim().parse().ok()).flatten()
    })
}

/// Every process's name, CPU time and resident memory now. One that ends
/// while it is read is left out.
pub fn processes() -> Vec<Sample> {
    let page = page_size();
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    dir.flatten()
        .filter_map(|entry| {
            let pid: u32 = entry.file_name().to_str()?.parse().ok()?;
            let (name, ticks, pages) = parse_stat(&read(&format!("/proc/{pid}/stat"))?)?;
            Some(Sample {
                pid,
                name,
                ticks,
                memory: pages * page,
            })
        })
        .collect()
}

/// The folder a process runs in, where this user may read it.
pub fn cwd(pid: u32) -> Option<String> {
    std::fs::read_link(format!("/proc/{pid}/cwd"))
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}

/// Clock ticks per second, the unit of a process's CPU time.
pub fn ticks_per_second() -> f32 {
    // SAFETY: sysconf reads a constant.
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if ticks > 0 { ticks as f32 } else { 100. }
}

fn page_size() -> u64 {
    // SAFETY: sysconf reads a constant.
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if page > 0 { page as u64 } else { 4096 }
}

/// A `/proc/<pid>/stat` line: the name, user and system ticks, and the
/// resident pages. The name is in parentheses and may hold anything, so
/// the fields are counted from its last `)`.
pub fn parse_stat(stat: &str) -> Option<(String, u64, u64)> {
    let open = stat.find('(')?;
    let close = stat.rfind(')')?;
    let name = stat.get(open + 1..close)?.to_owned();
    let fields: Vec<&str> = stat.get(close + 1..)?.split_whitespace().collect();
    // Field 3 (state) is fields[0]: utime is 14, stime 15, rss 24.
    let number = |i: usize| fields.get(i)?.parse::<u64>().ok();
    Some((name, number(11)? + number(12)?, number(21)?))
}

/// A `/proc/meminfo` field in bytes.
fn meminfo_field(meminfo: &str, name: &str) -> Option<u64> {
    meminfo
        .lines()
        .find_map(|line| line.strip_prefix(name)?.strip_prefix(':'))
        .and_then(|rest| rest.split_whitespace().next()?.parse::<u64>().ok())
        .map(|kib| kib * 1024)
}

/// Memory in use: total less available, in bytes.
pub fn parse_memory(meminfo: &str) -> Option<Usage> {
    let field = |name: &str| meminfo_field(meminfo, name);
    let total = field("MemTotal").filter(|&total| total > 0)?;
    let available = field("MemAvailable").unwrap_or_else(|| {
        field("MemFree").unwrap_or(0) + field("Buffers").unwrap_or(0) + field("Cached").unwrap_or(0)
    });
    Some(Usage {
        used: total.saturating_sub(available),
        total,
    })
}

/// Swap in use: total less free, in bytes. A machine without swap has a
/// total of 0.
pub fn parse_swap(meminfo: &str) -> Option<Usage> {
    let total = meminfo_field(meminfo, "SwapTotal")?;
    let free = meminfo_field(meminfo, "SwapFree").unwrap_or(total);
    Some(Usage {
        used: total.saturating_sub(free),
        total,
    })
}

/// Seconds since boot.
pub fn uptime() -> Option<u64> {
    parse_uptime(&read("/proc/uptime")?)
}

pub fn parse_uptime(uptime: &str) -> Option<u64> {
    let seconds: f64 = uptime.split_whitespace().next()?.parse().ok()?;
    Some(seconds as u64)
}

/// The first CPU's model name in `/proc/cpuinfo`.
pub fn parse_cpu_model(cpuinfo: &str) -> Option<String> {
    cpuinfo.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        (key.trim() == "model name").then(|| value.trim().to_owned())
    })
}

/// `PRETTY_NAME` from `/etc/os-release`, unquoted.
pub fn parse_os_name(os_release: &str) -> Option<String> {
    let value = os_release
        .lines()
        .find_map(|line| line.strip_prefix("PRETTY_NAME="))?;
    Some(value.trim().trim_matches('"').to_owned()).filter(|name| !name.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_counts_iowait_as_idle_and_guests_once() {
        let stat = "cpu  100 5 50 800 40 3 2 0 30 0\ncpu0 1 2 3 4 5 6 7 8 9 10\n";
        assert_eq!(
            parse_cpu(stat),
            Some(CpuTimes {
                total: 1000,
                idle: 840
            })
        );
    }

    #[test]
    fn memory_in_use_is_total_less_available() {
        let meminfo = "MemTotal:       16000 kB\nMemFree:  1000 kB\nMemAvailable:   12000 kB\n";
        assert_eq!(
            parse_memory(meminfo),
            Some(Usage {
                used: 4000 * 1024,
                total: 16000 * 1024
            })
        );
    }

    #[test]
    fn memory_without_available_adds_free_buffers_and_cache() {
        let meminfo = "MemTotal: 100 kB\nMemFree: 10 kB\nBuffers: 5 kB\nCached: 25 kB\n";
        assert_eq!(parse_memory(meminfo).map(|m| m.used), Some(60 * 1024));
        assert_eq!(parse_memory("MemFree: 10 kB\n"), None);
    }

    #[test]
    fn swap_in_use_is_total_less_free() {
        let meminfo = "MemTotal: 100 kB\nSwapTotal:  8000 kB\nSwapFree:  6000 kB\n";
        assert_eq!(
            parse_swap(meminfo),
            Some(Usage {
                used: 2000 * 1024,
                total: 8000 * 1024
            })
        );
        let none = "SwapTotal:  0 kB\nSwapFree:  0 kB\n";
        assert_eq!(parse_swap(none), Some(Usage { used: 0, total: 0 }));
        assert_eq!(parse_swap("MemTotal: 100 kB\n"), None);
    }

    #[test]
    fn vmstat_requires_both_swap_counters() {
        assert_eq!(
            parse_swap_pages("other 9\npswpin 100\npswpout 200\n"),
            Some((100, 200))
        );
        assert_eq!(parse_swap_pages("pswpin 100\n"), None);
    }

    #[test]
    fn a_stat_line_counts_its_fields_after_the_name() {
        let stat = "4242 (tmux: server) S 1 4242 4242 0 -1 4194560 300 0 0 0 \
                    150 50 0 0 20 0 1 0 900 12345678 2048 18446744073709551615";
        assert_eq!(
            parse_stat(stat),
            Some(("tmux: server".to_owned(), 200, 2048))
        );
        let odd = "7 (a) b (c)) R 1 7 7 0 -1 0 0 0 0 0 3 4 0 0 20 0 1 0 9 100 5";
        assert_eq!(parse_stat(odd), Some(("a) b (c)".to_owned(), 7, 5)));
        assert_eq!(parse_stat("7 (gone"), None);
    }

    #[test]
    fn oom_kills_come_from_vmstat() {
        assert_eq!(vmstat_field("pswpin 1\noom_kill 3\n", "oom_kill"), Some(3));
        assert_eq!(vmstat_field("pswpin 1\n", "oom_kill"), None);
    }

    #[test]
    fn uptime_is_whole_seconds() {
        assert_eq!(parse_uptime("350735.47 234388.90\n"), Some(350735));
        assert_eq!(parse_uptime(""), None);
    }

    #[test]
    fn machine_names_come_from_cpuinfo_and_os_release() {
        let cpuinfo = "processor\t: 0\nmodel name\t: AMD Ryzen 9 7950X 16-Core Processor\n";
        assert_eq!(
            parse_cpu_model(cpuinfo).as_deref(),
            Some("AMD Ryzen 9 7950X 16-Core Processor")
        );
        let os = "NAME=\"Debian GNU/Linux\"\nPRETTY_NAME=\"Debian GNU/Linux 12 (bookworm)\"\n";
        assert_eq!(
            parse_os_name(os).as_deref(),
            Some("Debian GNU/Linux 12 (bookworm)")
        );
        assert_eq!(parse_os_name("NAME=x\n"), None);
    }
}
