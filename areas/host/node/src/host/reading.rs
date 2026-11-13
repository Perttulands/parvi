//! One sample of the host, as the wire carries it.

use std::path::PathBuf;
use std::time::Instant;
use std::time::{SystemTime, UNIX_EPOCH};

use super::cpu::CpuTimes;
use super::disk::Mount;
use super::heavy::Before;
use super::{disk, gpu, os};

pub use parvi_host_contract::{Disk, Gpu, Reading, Usage};

/// Takes readings, remembering what the next is measured against: CPU
/// time, swap pages and each process's CPU time.
pub struct Probe {
    cpu: Option<CpuTimes>,
    nvidia_smi: Option<PathBuf>,
    mounts: Vec<Mount>,
    threads: usize,
    processes: Before,
    #[cfg(not(windows))]
    swap_sample: Option<((u64, u64), Instant)>,
}

impl Probe {
    /// A probe of `mounts` on a machine of `threads` CPUs.
    pub fn new(mounts: Vec<Mount>, threads: usize) -> Probe {
        Probe {
            cpu: os::cpu_times(),
            nvidia_smi: gpu::locate(),
            mounts,
            threads,
            processes: Before::default(),
            #[cfg(not(windows))]
            swap_sample: os::swap_pages().map(|pages| (pages, Instant::now())),
        }
    }

    /// The host now; with what is heavy if `heavy`. A reading without it
    /// forgets the processes, so the next with it does not measure CPU
    /// across the time nobody looked.
    pub async fn read(&mut self, heavy: bool) -> Reading {
        let cpu = os::cpu_times();
        let busy = cpu
            .zip(self.cpu)
            .and_then(|(now, then)| now.busy_since(then));
        self.cpu = cpu.or(self.cpu);
        let heavy = if heavy {
            self.processes.heaviest(
                os::processes(),
                Instant::now(),
                self.threads,
                os::ticks_per_second(),
                os::cwd,
            )
        } else {
            self.processes.forget();
            Vec::new()
        };
        let (gpus, gpu_reason) = match gpu::read(self.nvidia_smi.as_deref()).await {
            Ok(gpus) => (gpus, None),
            Err(reason) => (Vec::new(), Some(reason)),
        };
        #[cfg(not(windows))]
        let swap_activity_bps = {
            let sample = os::swap_pages().map(|pages| (pages, Instant::now()));
            let rate = sample.zip(self.swap_sample).and_then(|(now, previous)| {
                let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
                (page_size > 0)
                    .then(|| swap_rate(previous, now, page_size as u64))
                    .flatten()
            });
            self.swap_sample = sample;
            rate
        };
        #[cfg(windows)]
        let swap_activity_bps = None;
        let disks = self
            .mounts
            .iter()
            .filter_map(|m| {
                let usage = disk::usage(&m.path)?;
                Some(Disk {
                    name: m.name.clone(),
                    mount: m.path.clone(),
                    used: usage.used,
                    total: usage.total,
                })
            })
            .collect();
        Reading {
            at: now_ms(),
            cpu: busy,
            memory: os::memory(),
            swap: os::swap(),
            swap_activity_bps,
            gpu_reason,
            disks,
            gpus,
            uptime: os::uptime(),
            heavy,
        }
    }
}

#[cfg(not(windows))]
fn swap_rate(
    previous: ((u64, u64), Instant),
    now: ((u64, u64), Instant),
    page_size: u64,
) -> Option<f32> {
    let seconds = now.1.checked_duration_since(previous.1)?.as_secs_f32();
    if seconds == 0.0 {
        return None;
    }
    let pages = now
        .0
        .0
        .saturating_sub(previous.0.0)
        .saturating_add(now.0.1.saturating_sub(previous.0.1));
    Some(pages as f32 * page_size as f32 / seconds)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(windows))]
    #[test]
    fn swap_rates_measure_both_counters_over_elapsed_time() {
        let start = Instant::now();
        let later = start + std::time::Duration::from_secs(5);
        assert_eq!(
            swap_rate(((100, 200), start), ((1100, 2200), later), 4096),
            Some(2_457_600.0)
        );
        assert_eq!(
            swap_rate(((100, 200), start), ((100, 200), later), 4096),
            Some(0.0)
        );
    }

    #[cfg(any(target_os = "linux", windows))]
    #[tokio::test]
    async fn this_host_reads_cpu_memory_swap_disks_uptime_and_what_is_heavy() {
        let mut probe = Probe::new(disk::mounts(), 1);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let reading = probe.read(false).await;
        assert!(reading.memory.is_some_and(|m| m.used <= m.total));
        assert!(reading.swap.is_some_and(|s| s.used <= s.total));
        assert!(!reading.disks.is_empty());
        assert!(reading.uptime.is_some());
        assert!(reading.heavy.is_empty(), "not asked for");
        let next = probe.read(true).await;
        assert!(
            next.cpu.is_none_or(|cpu| (0.0..=100.0).contains(&cpu)),
            "{:?}",
            next.cpu
        );
        if cfg!(target_os = "linux") {
            assert!(!next.heavy.is_empty(), "this test is a process");
        }
    }
}
