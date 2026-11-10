//! CPU time, as Linux's `/proc/stat` and Windows' `GetSystemTimes` count it,
//! and the busy share between two counts.

/// Cumulative CPU time over every core, in the platform's units (clock
/// ticks on Linux, 100 ns on Windows).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuTimes {
    pub total: u64,
    pub idle: u64,
}

impl CpuTimes {
    /// The busy share of the time between `earlier` and `self`, in percent.
    /// `None` if no time passed.
    pub fn busy_since(self, earlier: CpuTimes) -> Option<f32> {
        let total = self.total.checked_sub(earlier.total)?;
        let idle = self.idle.saturating_sub(earlier.idle).min(total);
        (total > 0).then(|| (total - idle) as f32 / total as f32 * 100.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busy_share_comes_from_the_difference() {
        let earlier = CpuTimes {
            total: 1000,
            idle: 800,
        };
        let later = CpuTimes {
            total: 1200,
            idle: 950,
        };
        assert_eq!(later.busy_since(earlier), Some(25.0));
        assert_eq!(earlier.busy_since(earlier), None);
        assert_eq!(earlier.busy_since(later), None);
    }
}
