//! Drives `ButtonPolicy` with a scripted event stream, a fake uinput sink, a
//! fake restarter and a fake clock: no evdev, no threads sleeping for real.

use controller_agent::app::error::AppError;
use controller_agent::app::hub::RecordingSink;
use controller_agent::domain::keys::{KEY_1, KEY_4, KEY_ENTER, KEY_ESC, KEY_M};
use controller_agent::ports::{Clock, EventSink, FirstBootGate, KeyReplay, Restarter};
use controller_agent::protocol::BridgeEvent;
use controller_agent::services::input::{ButtonPolicy, NoGate};
use controller_agent::utils::config::Config;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Default)]
struct FakeUinput(Arc<Mutex<Vec<(u16, i32)>>>);
impl KeyReplay for FakeUinput {
    fn replay(&mut self, code: u16, value: i32) -> Result<(), AppError> {
        self.0.lock().unwrap().push((code, value));
        Ok(())
    }
}

#[derive(Default)]
struct FakeRestarter(Mutex<Vec<String>>);
impl Restarter for FakeRestarter {
    fn restart(&self, unit: &str) -> Result<(), AppError> {
        self.0.lock().unwrap().push(unit.to_string());
        Ok(())
    }
}

/// Time only moves when the test calls `advance`; `sleep_until` blocks until
/// the fake time reaches the deadline.
struct FakeClock {
    base: Instant,
    elapsed: Mutex<Duration>,
    cv: Condvar,
}
impl FakeClock {
    fn new() -> Arc<FakeClock> {
        Arc::new(FakeClock { base: Instant::now(), elapsed: Mutex::new(Duration::ZERO), cv: Condvar::new() })
    }
    fn advance(&self, d: Duration) {
        *self.elapsed.lock().unwrap() += d;
        self.cv.notify_all();
    }
}
impl Clock for FakeClock {
    fn now(&self) -> Instant {
        self.base + *self.elapsed.lock().unwrap()
    }
    fn sleep_until(&self, deadline: Instant) {
        let mut e = self.elapsed.lock().unwrap();
        while self.base + *e < deadline {
            e = self.cv.wait(e).unwrap();
        }
    }
}

/// A scripted first-boot gate: `active` is flipped by the test, `on_m_down`
/// calls are counted.
#[derive(Default)]
struct FakeGate {
    active: AtomicBool,
    m_downs: AtomicU32,
}
impl FirstBootGate for FakeGate {
    fn active(&self) -> bool {
        self.active.load(Ordering::SeqCst)
    }
    fn on_m_down(&self) {
        self.m_downs.fetch_add(1, Ordering::SeqCst);
    }
}

struct Rig {
    policy: ButtonPolicy,
    sink: Arc<RecordingSink>,
    uinput: FakeUinput,
    restarter: Arc<FakeRestarter>,
    clock: Arc<FakeClock>,
}

fn rig() -> Rig {
    rig_with(Arc::new(NoGate))
}

fn rig_with(gate: Arc<dyn FirstBootGate>) -> Rig {
    let sink = Arc::new(RecordingSink::new());
    let uinput = FakeUinput::default();
    let restarter = Arc::new(FakeRestarter::default());
    let clock = FakeClock::new();
    let input = Config::default().input;
    let policy = ButtonPolicy::new(
        sink.clone() as Arc<dyn EventSink>,
        Box::new(uinput.clone()),
        restarter.clone(),
        clock.clone(),
        input.restart_unit.clone(),
        Duration::from_millis(input.longpress_ms),
        gate,
    );
    Rig { policy, sink, uinput, restarter, clock }
}

fn buttons(sink: &RecordingSink) -> Vec<(u16, i32)> {
    sink.take()
        .into_iter()
        .map(|e| match e {
            BridgeEvent::Button { code, value } => (code, value),
            other => panic!("unexpected {other:?}"),
        })
        .collect()
}

#[test]
fn long_press_constant_is_1500_ms() {
    assert_eq!(Config::default().input.longpress_ms, 1500);
}

#[test]
fn every_key_is_broadcast_but_only_m_and_enter_are_replayed() {
    let mut r = rig();
    let script = [(KEY_ESC, 1), (KEY_ESC, 0), (KEY_1, 1), (KEY_4, 1), (KEY_ENTER, 1), (KEY_ENTER, 0), (200, 1)];
    for (c, v) in script {
        r.policy.handle(c, v);
    }
    assert_eq!(buttons(&r.sink), script, "all events broadcast unchanged, unknown codes included");
    assert_eq!(*r.uinput.0.lock().unwrap(), [(KEY_ENTER, 1), (KEY_ENTER, 0)]);
    assert!(r.restarter.0.lock().unwrap().is_empty());
}

#[test]
fn key_m_press_and_release_are_replayed_in_order_and_a_short_press_never_restarts() {
    let mut r = rig();
    let t = r.policy.handle(KEY_M, 1).expect("press starts a timer");
    r.clock.advance(Duration::from_millis(300));
    r.policy.handle(KEY_M, 0);
    r.clock.advance(Duration::from_secs(5));
    t.join().unwrap();
    assert_eq!(*r.uinput.0.lock().unwrap(), [(KEY_M, 1), (KEY_M, 0)]);
    assert!(r.restarter.0.lock().unwrap().is_empty());
}

#[test]
fn holding_m_past_the_threshold_restarts_the_unit_once_while_still_held() {
    let mut r = rig();
    let t = r.policy.handle(KEY_M, 1).unwrap();
    r.clock.advance(Duration::from_millis(1499));
    r.clock.advance(Duration::from_millis(1));
    t.join().unwrap();
    assert_eq!(*r.restarter.0.lock().unwrap(), ["thing-revived-terminal-agent.service"]);
    // The later release does not restart again.
    r.policy.handle(KEY_M, 0);
    assert_eq!(r.restarter.0.lock().unwrap().len(), 1);
}

#[test]
fn a_release_then_a_new_press_makes_the_old_timer_a_no_op() {
    let mut r = rig();
    let t1 = r.policy.handle(KEY_M, 1).unwrap();
    r.clock.advance(Duration::from_millis(1000));
    r.policy.handle(KEY_M, 0);
    let t2 = r.policy.handle(KEY_M, 1).unwrap();
    // t1's deadline passes now, but its press was superseded.
    r.clock.advance(Duration::from_millis(600));
    t1.join().unwrap();
    assert!(r.restarter.0.lock().unwrap().is_empty());
    // t2 fires 1500 ms after its own press.
    r.clock.advance(Duration::from_millis(900));
    t2.join().unwrap();
    assert_eq!(r.restarter.0.lock().unwrap().len(), 1);
}

// --- First-boot gate (delta plan 5.2) ---

fn gated() -> (Rig, Arc<FakeGate>) {
    let gate = Arc::new(FakeGate::default());
    gate.active.store(true, Ordering::SeqCst);
    (rig_with(gate.clone()), gate)
}

#[test]
fn an_always_inactive_gate_changes_nothing() {
    // The same stream as `every_key_is_broadcast_but_only_m_and_enter_are_replayed`
    // plus M, through a gate that is explicitly consulted and says no.
    let (mut r, gate) = gated();
    gate.active.store(false, Ordering::SeqCst);
    let t = r.policy.handle(KEY_M, 1).expect("an inactive gate leaves the timer alone");
    r.policy.handle(KEY_M, 0);
    r.clock.advance(Duration::from_secs(5));
    t.join().unwrap();
    r.policy.handle(KEY_ENTER, 1);
    r.policy.handle(KEY_ENTER, 0);
    assert_eq!(*r.uinput.0.lock().unwrap(), [(KEY_M, 1), (KEY_M, 0), (KEY_ENTER, 1), (KEY_ENTER, 0)]);
    assert_eq!(gate.m_downs.load(Ordering::SeqCst), 0);
    assert!(r.restarter.0.lock().unwrap().is_empty());
}

#[test]
fn an_active_gate_consumes_m_without_replay_or_timer_and_still_broadcasts() {
    let (mut r, gate) = gated();
    assert!(r.policy.handle(KEY_M, 1).is_none(), "no long-press timer is started");
    r.policy.handle(KEY_M, 0);
    assert_eq!(gate.m_downs.load(Ordering::SeqCst), 1);
    assert!(r.uinput.0.lock().unwrap().is_empty(), "no typed m");
    assert_eq!(buttons(&r.sink), [(KEY_M, 1), (KEY_M, 0)], "the raw events still reach the frontend");
}

#[test]
fn a_10_second_hold_while_active_never_restarts_the_agent_and_acks_once() {
    let (mut r, gate) = gated();
    assert!(r.policy.handle(KEY_M, 1).is_none());
    for _ in 0..10 {
        r.clock.advance(Duration::from_secs(1));
        assert!(r.policy.handle(KEY_M, 2).is_none(), "autorepeat is ignored");
    }
    r.policy.handle(KEY_M, 0);
    assert!(r.restarter.0.lock().unwrap().is_empty());
    assert_eq!(gate.m_downs.load(Ordering::SeqCst), 1, "one logical press");
    assert!(r.uinput.0.lock().unwrap().is_empty());
}

#[test]
fn a_press_consumed_while_active_suppresses_its_release_even_if_the_gate_goes_inactive() {
    // The second press acks, which makes the gate inactive before the release.
    let (mut r, gate) = gated();
    r.policy.handle(KEY_M, 1);
    gate.active.store(false, Ordering::SeqCst);
    r.policy.handle(KEY_M, 0);
    assert!(r.uinput.0.lock().unwrap().is_empty(), "no orphan m up reaches uinput");
    // The next press, now inactive, is an ordinary one again.
    let t = r.policy.handle(KEY_M, 1).expect("normal press starts a timer");
    r.policy.handle(KEY_M, 0);
    r.clock.advance(Duration::from_secs(5));
    t.join().unwrap();
    assert_eq!(*r.uinput.0.lock().unwrap(), [(KEY_M, 1), (KEY_M, 0)]);
    assert_eq!(gate.m_downs.load(Ordering::SeqCst), 1);
}

#[test]
fn a_press_that_began_inactive_is_not_consumed_on_release_when_the_gate_arms_mid_hold() {
    let (mut r, gate) = gated();
    gate.active.store(false, Ordering::SeqCst);
    let t = r.policy.handle(KEY_M, 1).expect("typed m, timer running");
    gate.active.store(true, Ordering::SeqCst);
    r.policy.handle(KEY_M, 0);
    r.clock.advance(Duration::from_secs(5));
    t.join().unwrap();
    assert_eq!(*r.uinput.0.lock().unwrap(), [(KEY_M, 1), (KEY_M, 0)], "the release is replayed: no stuck key");
    assert!(r.restarter.0.lock().unwrap().is_empty(), "and the release cancelled the timer");
    assert_eq!(gate.m_downs.load(Ordering::SeqCst), 0, "the press was never the gate's");
}

#[test]
fn an_active_gate_consumes_the_knob_push_on_the_press_edge_only() {
    let (mut r, gate) = gated();
    r.policy.handle(KEY_ENTER, 1);
    r.policy.handle(KEY_ENTER, 2);
    gate.active.store(false, Ordering::SeqCst);
    r.policy.handle(KEY_ENTER, 0);
    assert!(r.uinput.0.lock().unwrap().is_empty(), "no \\r typed, and the release has no orphan");
    assert_eq!(gate.m_downs.load(Ordering::SeqCst), 0, "Enter never advances the phase machine");
    // A push that began inactive and ends active is replayed in full.
    r.policy.handle(KEY_ENTER, 1);
    gate.active.store(true, Ordering::SeqCst);
    r.policy.handle(KEY_ENTER, 0);
    assert_eq!(*r.uinput.0.lock().unwrap(), [(KEY_ENTER, 1), (KEY_ENTER, 0)]);
}

#[test]
fn other_keys_are_untouched_by_an_active_gate() {
    let (mut r, _gate) = gated();
    for (c, v) in [(KEY_ESC, 1), (KEY_ESC, 0), (KEY_1, 1), (KEY_4, 1)] {
        r.policy.handle(c, v);
    }
    assert_eq!(buttons(&r.sink).len(), 4);
    assert!(r.uinput.0.lock().unwrap().is_empty());
}

#[test]
fn a_gate_that_goes_inactive_between_presses_restores_normal_long_press_restart() {
    let (mut r, gate) = gated();
    r.policy.handle(KEY_M, 1);
    r.policy.handle(KEY_M, 0);
    gate.active.store(false, Ordering::SeqCst);
    let t = r.policy.handle(KEY_M, 1).unwrap();
    r.clock.advance(Duration::from_millis(1500));
    t.join().unwrap();
    assert_eq!(r.restarter.0.lock().unwrap().len(), 1);
}
