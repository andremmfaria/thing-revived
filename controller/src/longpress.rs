//! Pure long-press decision logic for KEY_M, with injectable time so it can
//! be tested without evdev hardware.
//!
//! Each press gets a generation number. A timer started for press N may only
//! fire if press N is still the current, still-held press when its deadline
//! passes; a release or a newer press makes the older timer a silent no-op.

use std::time::{Duration, Instant};

pub struct LongPress {
    threshold: Duration,
    next_generation: u64,
    /// `(generation, deadline)` of the press currently held, if any.
    current: Option<(u64, Instant)>,
}

impl LongPress {
    pub fn new(threshold: Duration) -> Self {
        LongPress { threshold, next_generation: 0, current: None }
    }

    /// Registers a new press, superseding any earlier one. Returns the
    /// generation to hand to the timer and the moment it may fire.
    pub fn on_press(&mut self, now: Instant) -> (u64, Instant) {
        self.next_generation += 1;
        let deadline = now + self.threshold;
        self.current = Some((self.next_generation, deadline));
        (self.next_generation, deadline)
    }

    /// Key released: cancels the current press.
    pub fn on_release(&mut self) {
        self.current = None;
    }

    /// Called by the timer for `generation` at `now`. Returns true exactly
    /// once, and only if that press is still current and its deadline has
    /// passed; firing claims the press so a later release does not act on it.
    pub fn should_fire(&mut self, now: Instant, generation: u64) -> bool {
        match self.current {
            Some((g, deadline)) if g == generation && now >= deadline => {
                self.current = None;
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: Duration = Duration::from_millis(1500);

    #[test]
    fn release_before_deadline_cancels() {
        let mut lp = LongPress::new(T);
        let t0 = Instant::now();
        let (g, deadline) = lp.on_press(t0);
        assert_eq!(deadline, t0 + T);
        lp.on_release();
        assert!(!lp.should_fire(deadline, g));
    }

    #[test]
    fn newer_press_supersedes_older_timer() {
        let mut lp = LongPress::new(T);
        let t0 = Instant::now();
        let (g1, d1) = lp.on_press(t0);
        lp.on_release();
        let (g2, d2) = lp.on_press(t0 + Duration::from_millis(100));
        assert_ne!(g1, g2);
        // Old timer wakes at its deadline: second press is only 1400ms old.
        assert!(!lp.should_fire(d1, g1));
        assert!(!lp.should_fire(d1, g2), "second press not yet at its own deadline");
        assert!(lp.should_fire(d2, g2));
    }

    #[test]
    fn press_without_release_supersedes_too() {
        let mut lp = LongPress::new(T);
        let t0 = Instant::now();
        let (g1, _) = lp.on_press(t0);
        let (g2, d2) = lp.on_press(t0 + Duration::from_millis(10));
        assert!(!lp.should_fire(d2, g1));
        assert!(lp.should_fire(d2, g2));
    }

    #[test]
    fn fires_once_after_deadline_and_release_then_does_nothing() {
        let mut lp = LongPress::new(T);
        let t0 = Instant::now();
        let (g, deadline) = lp.on_press(t0);
        assert!(!lp.should_fire(deadline - Duration::from_millis(1), g), "too early");
        assert!(lp.should_fire(deadline, g));
        assert!(!lp.should_fire(deadline + Duration::from_secs(1), g), "only once");
        lp.on_release();
        assert!(!lp.should_fire(deadline + Duration::from_secs(2), g));
    }
}
