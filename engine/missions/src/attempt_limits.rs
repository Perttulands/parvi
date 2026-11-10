//! What the Limit cards covering one attempt allow it, as its executor sees
//! them: a flag the coordinator raises when a card's time runs out or the
//! attempt's seats spend its tokens, the warnings to paste into its seats,
//! and the meter its live token counts go to.
use crate::LimitWarning;
use std::collections::HashMap;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

/// An executor checks `spent` while its seats work; once it is raised the
/// attempt is over, so the executor ends its seats and returns.
#[derive(Clone, Debug, Default)]
pub struct AttemptLimits {
    pub spent: Arc<AtomicBool>,
    pub warnings: Vec<LimitWarning>,
    pub tokens: Option<Arc<TokenMeter>>,
    /// Raised with `spent` when a time card ran out, rather than tokens.
    pub(crate) timed_out: Arc<AtomicBool>,
}

impl AttemptLimits {
    pub fn is_spent(&self) -> bool {
        self.spent.load(Ordering::Acquire)
    }
    /// Raise `spent` because the attempt's time ran out.
    pub(crate) fn time_out(&self) {
        self.timed_out.store(true, Ordering::Release);
        self.spent.store(true, Ordering::Release);
    }
    pub(crate) fn timed_out(&self) -> bool {
        self.timed_out.load(Ordering::Acquire)
    }
    /// Whether a card counts this attempt's tokens, so its seats are metered.
    pub fn counts_tokens(&self) -> bool {
        self.tokens.is_some()
    }
    /// Record a dispatch's live count so far; raises `spent` once the attempt
    /// or the run's attempts together reach their card.
    pub fn count_tokens(&self, dispatch: &str, counted: i64) {
        if let Some(meter) = &self.tokens
            && meter.set(dispatch, counted)
        {
            self.spent.store(true, Ordering::Release);
        }
    }
    /// Record a dispatch's final count without stopping it: the work is
    /// done, so only the ledger needs to know.
    pub fn settle_tokens(&self, dispatch: &str, counted: i64) {
        if let Some(meter) = &self.tokens {
            meter.set(dispatch, counted);
        }
    }
    /// Each metered dispatch's last count.
    pub(crate) fn counted_tokens(&self) -> Vec<(String, i64)> {
        (self.tokens.iter())
            .flat_map(|meter| meter.counts.lock().unwrap().clone())
            .collect()
    }
}

/// Sums one attempt's live dispatch counts against what its tightest tokens
/// card had left when it started, and adds them to the run's pool.
#[derive(Debug)]
pub struct TokenMeter {
    budget: i64,
    pool: Option<Arc<TokenPool>>,
    counts: Mutex<HashMap<String, i64>>,
}

impl TokenMeter {
    pub(crate) fn new(budget: i64, pool: Option<Arc<TokenPool>>) -> Self {
        Self {
            budget,
            pool,
            counts: Mutex::default(),
        }
    }
    fn set(&self, dispatch: &str, counted: i64) -> bool {
        let pool_spent = self
            .pool
            .as_ref()
            .is_some_and(|pool| pool.set(dispatch, counted));
        let mut counts = self.counts.lock().unwrap();
        counts.insert(dispatch.to_owned(), counted);
        pool_spent || counts.values().sum::<i64>() >= self.budget
    }
    /// Whether the run's attempts together spent the mission's card.
    pub(crate) fn pool_spent(&self) -> bool {
        self.pool.as_ref().is_some_and(|pool| pool.spent())
    }
}

/// What a run's attempts spend while its worker works. Steps run at once and
/// the mission's tokens card covers them all, so the pool adds every live
/// dispatch count to what the ledger held when the worker began.
#[derive(Debug)]
pub(crate) struct TokenPool {
    max: i64,
    base: i64,
    counts: Mutex<HashMap<String, i64>>,
}

impl TokenPool {
    pub(crate) fn new(max: i64, base: i64) -> Self {
        Self {
            max,
            base,
            counts: Mutex::default(),
        }
    }
    fn set(&self, dispatch: &str, counted: i64) -> bool {
        let mut counts = self.counts.lock().unwrap();
        counts.insert(dispatch.to_owned(), counted);
        self.base + counts.values().sum::<i64>() >= self.max
    }
    fn spent(&self) -> bool {
        self.base + self.counts.lock().unwrap().values().sum::<i64>() >= self.max
    }
}
