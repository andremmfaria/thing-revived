//! Clock sync (design doc section 4), isolated from the PTY relay.
//!
//! Periodically asks a host (the dev workstation) for its time over ssh and
//! broadcasts the offset as `clock_sync` messages. It lives in this binary
//! only because this process already owns the client broadcast channel; it
//! shares nothing else with the PTY code and is skipped entirely when
//! `[clock] enabled = false`. Parsers and offset math are pure and unit
//! tested in `discovery` and `probe`.

pub mod discovery;
pub mod probe;

use crate::hub::{self, Clients};
use crate::protocol::{ClockStatus, ServerMsg};
use crate::utils::config::ClockConfig;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

pub struct ClockState {
    pub offset_ms: i64,
    pub status: ClockStatus,
    pub last_sync: Option<Instant>,
}

pub type SharedClock = Arc<Mutex<ClockState>>;

impl ClockState {
    pub fn new() -> Self {
        ClockState { offset_ms: 0, status: ClockStatus::Unsynced, last_sync: None }
    }

    /// The `clock_sync` message for this state at time `now`. `age_s` is
    /// present whenever the status is not `ok`.
    pub fn message(&self, now: Instant) -> ServerMsg {
        let age_s = (self.status != ClockStatus::Ok)
            .then(|| self.last_sync.map(|t| now.saturating_duration_since(t).as_secs()).unwrap_or(0));
        ServerMsg::ClockSync { offset_ms: self.offset_ms, status: self.status, age_s }
    }

    pub fn record_success(&mut self, offset_ms: i64, now: Instant) {
        self.offset_ms = offset_ms;
        self.status = ClockStatus::Ok;
        self.last_sync = Some(now);
    }

    /// Never zeroes a previously good offset: only flips to `stale`, and only
    /// if there ever was a good offset (otherwise it stays `unsynced`).
    pub fn record_failure(&mut self) {
        if self.last_sync.is_some() {
            self.status = ClockStatus::Stale;
        }
    }
}

impl Default for ClockState {
    fn default() -> Self {
        Self::new()
    }
}

fn lock(clock: &Mutex<ClockState>) -> MutexGuard<'_, ClockState> {
    clock.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn clock_sync_message(clock: &Mutex<ClockState>) -> ServerMsg {
    lock(clock).message(Instant::now())
}

/// Sleep before the next probe: the normal interval after success, else the
/// backoff table indexed by consecutive failures (last entry repeats).
pub fn next_delay(cfg: &ClockConfig, consecutive_failures: usize) -> Duration {
    if consecutive_failures == 0 {
        return Duration::from_secs(cfg.interval_secs);
    }
    let idx = (consecutive_failures - 1).min(cfg.backoff_secs.len().saturating_sub(1));
    Duration::from_secs(cfg.backoff_secs.get(idx).copied().unwrap_or(cfg.interval_secs))
}

/// Dedicated thread body: re-discovers the host, probes the offset and
/// broadcasts `clock_sync` to every client, forever.
pub fn run(clients: Clients, clock: SharedClock, cfg: ClockConfig) {
    let mut consecutive_failures: usize = 0;
    loop {
        let host = discovery::discover_host(&cfg);
        let result = host.as_deref().and_then(|h| probe::probe_offset_ms(&cfg, h));

        match result {
            Some(offset_ms) => {
                consecutive_failures = 0;
                lock(&clock).record_success(offset_ms, Instant::now());
                log::info!("clock_sync: ok, host={}, offset_ms={offset_ms}", host.as_deref().unwrap_or("?"));
            }
            None => {
                consecutive_failures += 1;
                lock(&clock).record_failure();
                log::warn!("clock_sync: probe failed (host={host:?}, consecutive_failures={consecutive_failures})");
            }
        }

        hub::broadcast(&clients, &clock_sync_message(&clock));
        thread::sleep(next_delay(&cfg, consecutive_failures));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn age_present_unless_ok_with_injected_now() {
        let t0 = Instant::now();
        let mut s = ClockState::new();
        assert_eq!(s.message(t0), ServerMsg::ClockSync { offset_ms: 0, status: ClockStatus::Unsynced, age_s: Some(0) });
        s.record_success(-42, t0);
        assert_eq!(s.message(t0 + Duration::from_secs(5)), ServerMsg::ClockSync { offset_ms: -42, status: ClockStatus::Ok, age_s: None });
        s.record_failure();
        assert_eq!(
            s.message(t0 + Duration::from_secs(125)),
            ServerMsg::ClockSync { offset_ms: -42, status: ClockStatus::Stale, age_s: Some(125) }
        );
    }

    #[test]
    fn failure_before_first_sync_stays_unsynced() {
        let mut s = ClockState::new();
        s.record_failure();
        assert_eq!(s.status, ClockStatus::Unsynced);
    }

    #[test]
    fn backoff_table() {
        let cfg = ClockConfig::default();
        let secs = |n| next_delay(&cfg, n).as_secs();
        assert_eq!(secs(0), 60);
        assert_eq!((secs(1), secs(2), secs(3)), (10, 20, 40));
        assert_eq!((secs(4), secs(100)), (40, 40));
    }
}
