//! One sample of the host, and the small point of it kept for history.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use super::cpu::CpuTimes;
use super::{disk, gpu, os};

pub use parvi_host_contract::{Gpu, Point, Usage};

/// Load averages: runnable and waiting tasks over 1, 5 and 15 minutes.
/// Linux only: Windows keeps none.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Load {
    pub one: f32,
    pub five: f32,
    pub fifteen: f32,
}

/// The host now. Anything that could not be read is `None` or empty.
#[derive(Debug, Clone, PartialEq)]
pub struct Reading {
    /// Unix ms.
    pub at: u64,
    /// Busy share of every core since the reading before, in percent.
    pub cpu: Option<f32>,
    pub memory: Option<Usage>,
    /// Swap in use; on Windows the commit charge against the commit limit
    /// (memory plus page files).
    pub swap: Option<Usage>,
    pub load: Option<Load>,
    /// The file system at [`disk::mount`].
    pub disk: Option<Usage>,
    pub gpus: Vec<Gpu>,
    /// Seconds since boot.
    pub uptime: Option<u64>,
}

/// The point of a reading on a machine of `threads` logical CPUs.
pub fn point_of(reading: &Reading, threads: usize) -> Point {
    let gpu = reading.gpus.first();
    Point {
        at: reading.at,
        cpu: reading.cpu,
        memory: reading.memory.and_then(Usage::percent),
        gpu: gpu.and_then(|gpu| gpu.util),
        load: reading
            .load
            .filter(|_| threads > 0)
            .map(|load| load.one / threads as f32 * 100.0),
        swap: reading.swap.and_then(Usage::percent),
        vram: gpu.and_then(|gpu| gpu.memory?.percent()),
    }
}

/// Takes readings, remembering CPU time between them.
pub struct Probe {
    cpu: Option<CpuTimes>,
    nvidia_smi: Option<PathBuf>,
}

impl Probe {
    pub fn new() -> Probe {
        Probe {
            cpu: os::cpu_times(),
            nvidia_smi: gpu::locate(),
        }
    }

    pub async fn read(&mut self) -> Reading {
        let cpu = os::cpu_times();
        let busy = cpu
            .zip(self.cpu)
            .and_then(|(now, then)| now.busy_since(then));
        self.cpu = cpu.or(self.cpu);
        let gpus = match &self.nvidia_smi {
            Some(path) => gpu::query(path).await,
            None => Vec::new(),
        };
        Reading {
            at: now_ms(),
            cpu: busy,
            memory: os::memory(),
            swap: os::swap(),
            load: os::load(),
            disk: disk::usage(&disk::mount()),
            gpus,
            uptime: os::uptime(),
        }
    }
}

impl Default for Probe {
    fn default() -> Probe {
        Probe::new()
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_point_keeps_percentages_and_the_first_gpu() {
        let reading = Reading {
            at: 7,
            cpu: Some(12.5),
            memory: Some(Usage { used: 1, total: 4 }),
            swap: Some(Usage { used: 1, total: 8 }),
            load: Some(Load {
                one: 4.0,
                five: 2.0,
                fifteen: 1.0,
            }),
            disk: None,
            gpus: vec![Gpu {
                name: "a".into(),
                util: Some(80.0),
                memory: Some(Usage { used: 3, total: 4 }),
                temp: None,
            }],
            uptime: Some(3),
        };
        assert_eq!(
            point_of(&reading, 16),
            Point {
                at: 7,
                cpu: Some(12.5),
                memory: Some(25.0),
                gpu: Some(80.0),
                load: Some(25.0),
                swap: Some(12.5),
                vram: Some(75.0),
            }
        );
    }

    #[test]
    fn no_swap_and_no_load_are_unknown_not_zero() {
        let reading = Reading {
            at: 7,
            cpu: None,
            memory: None,
            swap: Some(Usage { used: 0, total: 0 }),
            load: None,
            disk: None,
            gpus: Vec::new(),
            uptime: None,
        };
        let point = point_of(&reading, 16);
        assert_eq!((point.swap, point.load), (None, None));
    }

    #[cfg(any(target_os = "linux", windows))]
    #[tokio::test]
    async fn this_host_reads_cpu_memory_swap_disk_and_uptime() {
        let mut probe = Probe::new();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let reading = probe.read().await;
        assert!(reading.memory.is_some_and(|m| m.used <= m.total));
        assert!(reading.swap.is_some_and(|s| s.used <= s.total));
        assert_eq!(reading.load.is_some(), cfg!(target_os = "linux"));
        assert!(reading.disk.is_some());
        assert!(reading.uptime.is_some());
        let cpu = probe.read().await.cpu;
        assert!(
            cpu.is_none_or(|cpu| (0.0..=100.0).contains(&cpu)),
            "{cpu:?}"
        );
    }
}
