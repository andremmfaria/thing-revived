//! Unlock and mutation rate limiting (plan section 6). Time-injected state
//! machines: callers pass monotonic seconds, nothing sleeps.
//!
//! Unlock: per connection, `free_attempts` failures cost nothing, then a wait
//! of 5 s, 10 s, 20 s ... capped at `backoff_max_secs`, reset on success. A
//! global cap of `global_cap` failures per `global_window_secs` across all
//! connections; past it an attempt needs a fresh physical press (`note_press`,
//! one press allows one attempt) so a local process can stall other
//! connections but never the person at the device.

use std::collections::{HashMap, VecDeque};

/// Opaque connection key (the hub's connection id, as a plain number).
pub type ConnKey = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnlockConfig {
    pub free_attempts: u32,
    pub backoff_base_secs: u64,
    pub backoff_max_secs: u64,
    pub global_cap: usize,
    pub global_window_secs: u64,
}

impl Default for UnlockConfig {
    fn default() -> Self {
        UnlockConfig {
            free_attempts: 3,
            backoff_base_secs: 5,
            backoff_max_secs: 300,
            global_cap: 20,
            global_window_secs: 600,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Denied {
    /// This connection must wait this many more seconds.
    Wait(u64),
    /// The global cap is hit: a fresh physical press is required.
    NeedPress,
}

#[derive(Debug, Clone, Copy)]
struct ConnState {
    failures: u32,
    last_failure: u64,
}

#[derive(Debug)]
pub struct UnlockLimiter {
    cfg: UnlockConfig,
    conns: HashMap<ConnKey, ConnState>,
    global: VecDeque<u64>,
    press: bool,
}

impl UnlockLimiter {
    pub fn new(cfg: UnlockConfig) -> UnlockLimiter {
        UnlockLimiter { cfg, conns: HashMap::new(), global: VecDeque::new(), press: false }
    }

    fn prune(&mut self, now: u64) {
        let window = self.cfg.global_window_secs;
        while self.global.front().is_some_and(|t| now.saturating_sub(*t) >= window) {
            self.global.pop_front();
        }
    }

    fn wait_secs(&self, st: &ConnState) -> u64 {
        if st.failures <= self.cfg.free_attempts {
            return 0;
        }
        let exp = (st.failures - self.cfg.free_attempts - 1).min(32);
        self.cfg.backoff_base_secs.saturating_mul(1u64 << exp).min(self.cfg.backoff_max_secs)
    }

    /// Call before verifying a password. `Ok` consumes a pending press when
    /// the global cap needed one. A denied call changes nothing and does not
    /// count as a failure.
    pub fn attempt(&mut self, conn: ConnKey, now: u64) -> Result<(), Denied> {
        self.prune(now);
        if let Some(st) = self.conns.get(&conn) {
            let wait = self.wait_secs(st);
            let elapsed = now.saturating_sub(st.last_failure);
            if elapsed < wait {
                return Err(Denied::Wait(wait - elapsed));
            }
        }
        if self.global.len() >= self.cfg.global_cap {
            if !self.press {
                return Err(Denied::NeedPress);
            }
            self.press = false;
        }
        Ok(())
    }

    pub fn record_failure(&mut self, conn: ConnKey, now: u64) {
        self.prune(now);
        let st = self.conns.entry(conn).or_insert(ConnState { failures: 0, last_failure: now });
        st.failures = st.failures.saturating_add(1);
        st.last_failure = now;
        self.global.push_back(now);
    }

    /// Success clears this connection's backoff. The global window keeps its
    /// history so a guesser cannot launder failures by guessing right once.
    pub fn record_success(&mut self, conn: ConnKey) {
        self.conns.remove(&conn);
    }

    /// A physical press on the device (the same source as the old reveal).
    pub fn note_press(&mut self) {
        self.press = true;
    }

    pub fn forget(&mut self, conn: ConnKey) {
        self.conns.remove(&conn);
    }
}

/// Sliding window cap on mutating commands per connection.
#[derive(Debug)]
pub struct MutationLimiter {
    per_window: usize,
    window_secs: u64,
    seen: HashMap<ConnKey, VecDeque<u64>>,
}

impl MutationLimiter {
    pub fn new(per_window: usize, window_secs: u64) -> MutationLimiter {
        MutationLimiter { per_window, window_secs, seen: HashMap::new() }
    }

    /// 10 per minute, the plan default.
    pub fn per_minute(n: usize) -> MutationLimiter {
        MutationLimiter::new(n, 60)
    }

    /// True and counted if allowed; false and not counted when over the cap.
    pub fn allow(&mut self, conn: ConnKey, now: u64) -> bool {
        let q = self.seen.entry(conn).or_default();
        while q.front().is_some_and(|t| now.saturating_sub(*t) >= self.window_secs) {
            q.pop_front();
        }
        if q.len() >= self.per_window {
            return false;
        }
        q.push_back(now);
        true
    }

    pub fn forget(&mut self, conn: ConnKey) {
        self.seen.remove(&conn);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fail(l: &mut UnlockLimiter, c: ConnKey, now: u64) {
        assert_eq!(l.attempt(c, now), Ok(()));
        l.record_failure(c, now);
    }

    #[test]
    fn three_free_then_doubling_to_the_cap() {
        let mut l = UnlockLimiter::new(UnlockConfig::default());
        let mut t = 0;
        for _ in 0..3 {
            fail(&mut l, 1, t);
        }
        assert_eq!(l.attempt(1, t), Ok(()), "three failures cost nothing");
        l.record_failure(1, t); // 4th
        assert_eq!(l.attempt(1, t), Err(Denied::Wait(5)));
        assert_eq!(l.attempt(1, t + 3), Err(Denied::Wait(2)));
        t += 5;
        assert_eq!(l.attempt(1, t), Ok(()));
        l.record_failure(1, t); // 5th: 10 s
        assert_eq!(l.attempt(1, t + 9), Err(Denied::Wait(1)));
        t += 10;
        l.record_failure(1, t); // 6th: 20 s
        assert_eq!(l.attempt(1, t), Err(Denied::Wait(20)));
        // far into the sequence the wait stays at 300 s
        let mut l = UnlockLimiter::new(UnlockConfig { global_cap: 1000, ..UnlockConfig::default() });
        for i in 0..40 {
            l.record_failure(1, i);
        }
        assert_eq!(l.attempt(1, 40), Err(Denied::Wait(299)));
    }

    #[test]
    fn success_resets_the_connection() {
        let mut l = UnlockLimiter::new(UnlockConfig::default());
        for _ in 0..5 {
            l.record_failure(1, 0);
        }
        assert!(l.attempt(1, 0).is_err());
        l.record_success(1);
        assert_eq!(l.attempt(1, 0), Ok(()));
    }

    #[test]
    fn connections_are_independent_below_the_global_cap() {
        let mut l = UnlockLimiter::new(UnlockConfig::default());
        for _ in 0..5 {
            l.record_failure(1, 0);
        }
        assert!(l.attempt(1, 0).is_err());
        assert_eq!(l.attempt(2, 0), Ok(()));
    }

    #[test]
    fn global_cap_needs_one_press_per_attempt() {
        let mut l = UnlockLimiter::new(UnlockConfig::default());
        // 20 failures spread over many connections, each within its free tries
        for i in 0..20u64 {
            l.record_failure(100 + i, i);
        }
        assert_eq!(l.attempt(1, 20), Err(Denied::NeedPress));
        l.note_press();
        assert_eq!(l.attempt(1, 20), Ok(()), "a press allows one attempt");
        assert_eq!(l.attempt(1, 20), Err(Denied::NeedPress), "the press was consumed");
        // a denied attempt must not eat a press that is not there; a success clears nothing global
        l.record_success(1);
        assert_eq!(l.attempt(1, 20), Err(Denied::NeedPress));
    }

    #[test]
    fn global_window_slides() {
        let mut l = UnlockLimiter::new(UnlockConfig::default());
        for i in 0..20u64 {
            l.record_failure(100 + i, i);
        }
        assert_eq!(l.attempt(1, 599), Err(Denied::NeedPress));
        assert_eq!(l.attempt(1, 600), Ok(()), "the oldest failure aged out");
    }

    #[test]
    fn forget_drops_the_connection() {
        let mut l = UnlockLimiter::new(UnlockConfig::default());
        for _ in 0..5 {
            l.record_failure(1, 0);
        }
        l.forget(1);
        assert_eq!(l.attempt(1, 0), Ok(()));
    }

    #[test]
    fn mutation_window() {
        let mut m = MutationLimiter::per_minute(3);
        assert!(m.allow(1, 0) && m.allow(1, 10) && m.allow(1, 20));
        assert!(!m.allow(1, 30));
        assert!(m.allow(2, 30), "other connection unaffected");
        assert!(m.allow(1, 60), "first entry aged out");
        assert!(!m.allow(1, 61));
        m.forget(1);
        assert!(m.allow(1, 61));
    }
}
