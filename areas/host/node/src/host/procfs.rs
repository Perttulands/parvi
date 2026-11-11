//! Readings from Linux's `/proc`: CPU time, memory, swap, load, uptime and
//! what the machine is. Parsing is apart from reading, so the parsers are
//! tested on fixed text. Ported from CHROTE's `system.go`, which counts
//! iowait as idle and falls back from `MemAvailable` on old kernels.

use super::cpu::CpuTimes;
use super::reading::{Load, Usage};

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

pub fn load() -> Option<Load> {
    parse_load(&read("/proc/loadavg")?)
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

/// The 1, 5 and 15 minute load averages.
pub fn parse_load(loadavg: &str) -> Option<Load> {
    let mut fields = loadavg.split_whitespace().map(str::parse::<f32>);
    let mut next = || fields.next()?.ok();
    Some(Load {
        one: next()?,
        five: next()?,
        fifteen: next()?,
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
    fn load_is_the_first_three_averages() {
        assert_eq!(
            parse_load("0.42 0.73 0.62 2/1234 5678\n"),
            Some(Load {
                one: 0.42,
                five: 0.73,
                fifteen: 0.62
            })
        );
        assert_eq!(parse_load("0.42 x\n"), None);
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
