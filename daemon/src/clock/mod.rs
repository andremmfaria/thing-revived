//! Clock sync status (design doc section 4), isolated from the PTY relay.
//!
//! The device syncs its system clock itself (systemd-timesyncd, NTP over
//! whatever network is up), so the system clock is the reference and the
//! offset is always 0. This module only reports whether the system says the
//! clock is NTP synchronized, as `clock_sync` messages. It lives in this
//! binary only because this process already owns the client broadcast
//! channel; it shares nothing else with the PTY code and is skipped entirely
//! when `[clock] enabled = false`. The state source is a trait so tests need
//! no `timedatectl`.

pub mod source;

use crate::hub::ClientHub;
use source::{ClockSource, SourceError, TimedatectlSource};
use crate::protocol::ServerMsg;
use crate::utils::config::ClockConfig;
use crate::utils::types::{ClockStatus, SharedClock};
use std::sync::{Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

pub struct ClockState {
    pub offset_ms: i64,
    pub status: ClockStatus,
    pub last_sync: Option<Instant>,
}

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

/// One poll: folds the source's answer into `clock`. Returns whether the
/// clock is synchronized, which decides the next delay.
pub fn poll_once(source: &dyn ClockSource, clock: &Mutex<ClockState>, now: Instant) -> bool {
    match source.is_synchronized() {
        Ok(true) => {
            // The system clock is the reference, so there is no offset to report.
            lock(clock).record_success(0, now);
            true
        }
        Ok(false) => {
            lock(clock).record_failure();
            log::debug!("clock_sync: system clock not synchronized yet");
            false
        }
        Err(SourceError(reason)) => {
            lock(clock).record_failure();
            log::warn!("clock_sync: cannot read clock state: {reason}");
            false
        }
    }
}

/// Dedicated thread body: polls the system clock state and broadcasts
/// `clock_sync` to every client, forever. Polls quickly (the backoff table)
/// until the clock is synchronized so the status bar flips soon after the
/// network comes up, then at `interval_secs`.
pub fn run(clients: ClientHub, clock: SharedClock, cfg: ClockConfig) {
    let source = TimedatectlSource;
    let mut not_synced_polls: usize = 0;
    let mut last_broadcast: Option<ServerMsg> = None;
    loop {
        if poll_once(&source, &clock, Instant::now()) {
            not_synced_polls = 0;
        } else {
            not_synced_polls += 1;
        }

        let msg = clock_sync_message(&clock);
        // `age_s` of a stale clock changes every poll; unsynced and ok do not,
        // so only repeat identical messages at the slow cadence.
        if last_broadcast.as_ref() != Some(&msg) || not_synced_polls == 0 {
            clients.broadcast(&msg);
        }
        last_broadcast = Some(msg);
        thread::sleep(next_delay(&cfg, not_synced_polls));
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

    struct FakeSource(Result<bool, String>);

    impl ClockSource for FakeSource {
        fn is_synchronized(&self) -> Result<bool, SourceError> {
            self.0.clone().map_err(SourceError)
        }
    }

    fn msg(clock: &Mutex<ClockState>, now: Instant) -> ServerMsg {
        lock(clock).message(now)
    }

    #[test]
    fn synchronized_reports_ok_with_zero_offset() {
        let (clock, t0) = (Mutex::new(ClockState::new()), Instant::now());
        assert!(poll_once(&FakeSource(Ok(true)), &clock, t0));
        assert_eq!(msg(&clock, t0), ServerMsg::ClockSync { offset_ms: 0, status: ClockStatus::Ok, age_s: None });
    }

    #[test]
    fn unsynchronized_before_first_sync_reports_unsynced() {
        let (clock, t0) = (Mutex::new(ClockState::new()), Instant::now());
        assert!(!poll_once(&FakeSource(Ok(false)), &clock, t0));
        assert_eq!(msg(&clock, t0), ServerMsg::ClockSync { offset_ms: 0, status: ClockStatus::Unsynced, age_s: Some(0) });
    }

    #[test]
    fn command_failure_before_first_sync_reports_unsynced() {
        let (clock, t0) = (Mutex::new(ClockState::new()), Instant::now());
        assert!(!poll_once(&FakeSource(Err("no such file".into())), &clock, t0));
        assert_eq!(msg(&clock, t0), ServerMsg::ClockSync { offset_ms: 0, status: ClockStatus::Unsynced, age_s: Some(0) });
    }

    #[test]
    fn losing_sync_or_failing_after_ok_goes_stale_then_recovers() {
        let (clock, t0) = (Mutex::new(ClockState::new()), Instant::now());
        poll_once(&FakeSource(Ok(true)), &clock, t0);
        let later = t0 + Duration::from_secs(90);
        poll_once(&FakeSource(Err("boom".into())), &clock, later);
        assert_eq!(msg(&clock, later), ServerMsg::ClockSync { offset_ms: 0, status: ClockStatus::Stale, age_s: Some(90) });
        poll_once(&FakeSource(Ok(false)), &clock, later);
        assert_eq!(lock(&clock).status, ClockStatus::Stale);
        poll_once(&FakeSource(Ok(true)), &clock, later);
        assert_eq!(msg(&clock, later), ServerMsg::ClockSync { offset_ms: 0, status: ClockStatus::Ok, age_s: None });
    }
}
