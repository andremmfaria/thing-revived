//! Bounded auto-respawn policy (design doc section 8), pure and clock-free.
//!
//! Up to `max` involuntary exits within a trailing `window` get an automatic
//! retry with an increasing delay; the next one inside the window gives up
//! and the user must respawn manually. Manual respawns never reach this
//! type, so repeated manual respawns can never trip the limiter.

use crate::utils::config::RespawnConfig;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Respawn after `delay`; `attempt` is the 1-based count within the window.
    Retry { attempt: usize, delay: Duration },
    /// Crash-looping: stop auto-respawning; `attempt` exits within the window.
    GiveUp { attempt: usize },
}

#[derive(Debug, Clone)]
pub struct RespawnPolicy {
    window: Duration,
    max: usize,
    delays: Vec<Duration>,
    exits: Vec<Instant>,
}

impl RespawnPolicy {
    pub fn new(cfg: &RespawnConfig) -> Self {
        RespawnPolicy {
            window: Duration::from_secs(cfg.window_secs),
            max: cfg.max,
            delays: cfg.delays_secs.iter().map(|s| Duration::from_secs(*s)).collect(),
            exits: Vec::new(),
        }
    }

    pub fn window(&self) -> Duration {
        self.window
    }

    /// Exits currently inside the window (as of the last `on_exit`).
    pub fn recent_exits(&self) -> usize {
        self.exits.len()
    }

    /// Records one involuntary exit at `now` and decides what to do. The
    /// delay table is indexed by attempt; its last entry repeats if it is
    /// shorter than `max`.
    pub fn on_exit(&mut self, now: Instant) -> Decision {
        let window = self.window;
        self.exits.retain(|t| now.saturating_duration_since(*t) <= window);
        self.exits.push(now);
        let attempt = self.exits.len();
        if attempt <= self.max {
            let delay = self
                .delays
                .get(attempt - 1)
                .or(self.delays.last())
                .copied()
                .unwrap_or_default();
            Decision::Retry { attempt, delay }
        } else {
            Decision::GiveUp { attempt }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> RespawnPolicy {
        RespawnPolicy::new(&RespawnConfig::default())
    }

    fn retry(attempt: usize, secs: u64) -> Decision {
        Decision::Retry { attempt, delay: Duration::from_secs(secs) }
    }

    /// (seconds since t0 of each exit, expected decision for that exit)
    fn run(p: &mut RespawnPolicy, t0: Instant, table: &[(u64, Decision)]) {
        for (i, (at, want)) in table.iter().enumerate() {
            assert_eq!(p.on_exit(t0 + Duration::from_secs(*at)), *want, "row {i} at {at}s");
        }
    }

    #[test]
    fn three_fast_exits_retry_with_escalating_delay_then_give_up() {
        let t0 = Instant::now();
        run(&mut policy(), t0, &[
            (0, retry(1, 0)),
            (1, retry(2, 2)),
            (2, retry(3, 5)),
            (3, Decision::GiveUp { attempt: 4 }),
            (4, Decision::GiveUp { attempt: 5 }),
        ]);
    }

    #[test]
    fn old_exits_age_out_of_the_window() {
        let t0 = Instant::now();
        run(&mut policy(), t0, &[
            (0, retry(1, 0)),
            (10, retry(2, 2)),
            (20, retry(3, 5)),
            // t=31: the exit at 0 is 31s old (> 30s window), so only 3 count.
            (31, retry(3, 5)),
            // t=45: exits at 10 and 20 and 31 remain, 0 gone; 10 is 35s old and gone too.
            (45, retry(3, 5)),
        ]);
    }

    #[test]
    fn exactly_on_the_window_boundary_still_counts() {
        let t0 = Instant::now();
        run(&mut policy(), t0, &[(0, retry(1, 0)), (30, retry(2, 2)), (31, retry(2, 2))]);
    }

    #[test]
    fn quiet_period_resets_the_attempt_counter() {
        let t0 = Instant::now();
        run(&mut policy(), t0, &[
            (0, retry(1, 0)),
            (1, retry(2, 2)),
            (2, retry(3, 5)),
            (3, Decision::GiveUp { attempt: 4 }),
            (200, retry(1, 0)),
        ]);
    }

    #[test]
    fn short_delay_table_repeats_last_entry() {
        let cfg = RespawnConfig { window_secs: 30, max: 4, delays_secs: vec![1, 9] };
        let mut p = RespawnPolicy::new(&cfg);
        let t0 = Instant::now();
        run(&mut p, t0, &[(0, retry(1, 1)), (1, retry(2, 9)), (2, retry(3, 9)), (3, retry(4, 9)), (4, Decision::GiveUp { attempt: 5 })]);
    }

    #[test]
    fn max_zero_never_retries() {
        let cfg = RespawnConfig { max: 0, ..RespawnConfig::default() };
        let mut p = RespawnPolicy::new(&cfg);
        assert_eq!(p.on_exit(Instant::now()), Decision::GiveUp { attempt: 1 });
    }

    #[test]
    fn recent_exits_tracks_window() {
        let mut p = policy();
        let t0 = Instant::now();
        p.on_exit(t0);
        p.on_exit(t0 + Duration::from_secs(1));
        assert_eq!(p.recent_exits(), 2);
        p.on_exit(t0 + Duration::from_secs(100));
        assert_eq!(p.recent_exits(), 1);
    }
}
