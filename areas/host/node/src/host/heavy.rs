//! What is heavy now: every process's CPU time and memory, read twice,
//! made into the few programs that weigh most. Processes running one
//! program in one folder count as one (a build's twelve `rustc`), since that
//! is what the operator acts on: which work, not which pid.

use std::collections::HashMap;
use std::time::Instant;

pub use parvi_host_contract::Heavy;

/// One process as read.
#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    pub pid: u32,
    pub name: String,
    /// User and system CPU time so far, in the platform's ticks.
    pub ticks: u64,
    /// Resident bytes.
    pub memory: u64,
}

/// Processes looked at closely (their folder read) by memory and by CPU.
const CANDIDATES: usize = 48;
/// Programs kept by memory and by CPU; a program in both counts once.
pub const KEPT: usize = 6;

/// The processes' CPU time at the reading before, to measure the next
/// against.
#[derive(Default)]
pub struct Before {
    ticks: HashMap<u32, u64>,
    at: Option<Instant>,
}

impl Before {
    /// Forgets the reading before: the next one has no CPU to tell.
    pub fn forget(&mut self) {
        self.ticks.clear();
        self.at = None;
    }

    /// The heaviest programs in `samples`, read at `now` on a machine of
    /// `threads` CPUs counting `per_second` ticks a second. `cwd` reads a
    /// process's folder, asked only of the few that may make the list.
    pub fn heaviest(
        &mut self,
        samples: Vec<Sample>,
        now: Instant,
        threads: usize,
        per_second: f32,
        cwd: impl Fn(u32) -> Option<String>,
    ) -> Vec<Heavy> {
        let seconds = self.at.map(|at| now.duration_since(at).as_secs_f32());
        let capacity = seconds
            .filter(|s| *s > 0.)
            .map(|s| s * per_second * threads.max(1) as f32);
        let used: Vec<(Sample, Option<u64>)> = samples
            .into_iter()
            .map(|s| {
                let delta = capacity
                    .and(self.ticks.get(&s.pid))
                    .map(|before| s.ticks.saturating_sub(*before));
                (s, delta)
            })
            .collect();
        self.ticks = used.iter().map(|(s, _)| (s.pid, s.ticks)).collect();
        self.at = Some(now);

        let mut candidates: Vec<usize> = top(&used, CANDIDATES, |(s, _)| s.memory);
        for i in top(&used, CANDIDATES, |(_, d)| d.unwrap_or(0)) {
            if !candidates.contains(&i) {
                candidates.push(i);
            }
        }
        let mut groups: HashMap<(String, Option<String>), Heavy> = HashMap::new();
        for i in candidates {
            let (s, delta) = &used[i];
            let key = (s.name.clone(), cwd(s.pid));
            let group = groups.entry(key.clone()).or_insert_with(|| Heavy {
                name: key.0,
                cwd: key.1,
                cpu: capacity.map(|_| 0.),
                ..Heavy::default()
            });
            group.memory += s.memory;
            group.pids.push(s.pid);
            if let (Some(cpu), Some(delta), Some(capacity)) = (&mut group.cpu, delta, capacity) {
                *cpu += *delta as f32 / capacity * 100.;
            }
        }
        let groups: Vec<Heavy> = groups
            .into_values()
            .map(|mut g| {
                g.cpu = g.cpu.map(parvi_host_contract::tenth);
                g
            })
            .collect();
        let mut kept = top(&groups, KEPT, |g| g.memory);
        for i in top(&groups, KEPT, |g| (g.cpu.unwrap_or(0.) * 10.) as u64) {
            if !kept.contains(&i) && groups[i].cpu.is_some_and(|c| c > 0.) {
                kept.push(i);
            }
        }
        let mut out: Vec<Heavy> = kept.into_iter().map(|i| groups[i].clone()).collect();
        out.sort_by_key(|h| std::cmp::Reverse(h.memory));
        out
    }
}

/// The indices of the `n` largest by `key`, largest first.
fn top<T>(items: &[T], n: usize, key: impl Fn(&T) -> u64) -> Vec<usize> {
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by_key(|i| std::cmp::Reverse(key(&items[*i])));
    order.truncate(n);
    order
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn s(pid: u32, name: &str, ticks: u64, memory: u64) -> Sample {
        Sample {
            pid,
            name: name.into(),
            ticks,
            memory,
        }
    }

    fn folder(pid: u32) -> Option<String> {
        Some(if pid < 10 { "/srv/a" } else { "/srv/b" }.into())
    }

    #[test]
    fn one_program_in_one_folder_counts_once_and_cpu_needs_a_reading_before() {
        let mut before = Before::default();
        let start = Instant::now();
        let first = vec![
            s(1, "rustc", 0, 300),
            s(2, "rustc", 0, 300),
            s(11, "rustc", 0, 100),
            s(12, "claude", 0, 500),
        ];
        let heavy = before.heaviest(first, start, 2, 100., folder);
        assert_eq!(heavy.len(), 3);
        assert_eq!(heavy[0].name, "rustc");
        assert_eq!(heavy[0].memory, 600, "two rustc in /srv/a");
        assert!(heavy.iter().all(|h| h.cpu.is_none()), "no reading before");

        // Two seconds on two CPUs is 400 ticks of capacity.
        let later = vec![
            s(1, "rustc", 200, 300),
            s(2, "rustc", 100, 300),
            s(11, "rustc", 0, 100),
            s(12, "claude", 20, 500),
        ];
        let heavy = before.heaviest(later, start + Duration::from_secs(2), 2, 100., folder);
        let a = heavy
            .iter()
            .find(|h| h.cwd.as_deref() == Some("/srv/a"))
            .unwrap();
        assert_eq!(a.cpu, Some(75.));
        let claude = heavy.iter().find(|h| h.name == "claude").unwrap();
        assert_eq!(claude.cpu, Some(5.));

        before.forget();
        let again = before.heaviest(
            vec![s(1, "rustc", 900, 300)],
            start + Duration::from_secs(4),
            2,
            100.,
            folder,
        );
        assert_eq!(again[0].cpu, None, "forgotten: no CPU to tell");
    }

    #[test]
    fn keeps_the_heaviest_by_memory_and_the_busiest_by_cpu() {
        let mut before = Before::default();
        let start = Instant::now();
        let mut samples: Vec<Sample> = (0..20)
            .map(|i| s(100 + i, &format!("p{i}"), 0, 1000 - u64::from(i)))
            .collect();
        samples.push(s(9, "busy", 0, 1));
        before.heaviest(samples.clone(), start, 1, 100., |_| None);
        for sample in &mut samples {
            if sample.name == "busy" {
                sample.ticks = 100;
            }
        }
        let heavy = before.heaviest(samples, start + Duration::from_secs(1), 1, 100., |_| None);
        assert_eq!(heavy.len(), KEPT + 1);
        assert_eq!(heavy[0].name, "p0");
        assert_eq!(heavy.last().unwrap().name, "busy");
        assert_eq!(heavy.last().unwrap().cpu, Some(100.));
    }
}
