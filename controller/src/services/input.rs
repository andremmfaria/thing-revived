//! Button policy for the grabbed buttons device. Every key event is
//! broadcast unchanged; M and Enter are additionally replayed; M also feeds
//! the long-press detector, whose firing restarts a systemd unit directly
//! (no WebSocket round trip, no dependency on the browser or terminal agent).
//!
//! While the first-boot overlay is up (`FirstBootGate::active`), M and Enter
//! (the knob push) are consumed instead: no replay, no long-press timer. The
//! decision is taken on the press edge and remembered until the release, so a
//! release never arrives without its press and the gate flipping mid-hold
//! cannot leave a stuck or orphaned key (delta plan 5.2).

use crate::domain::keys::{is_known, is_replayed, KEY_ENTER, KEY_M};
use crate::domain::longpress::LongPress;
use crate::ports::{Clock, EventSink, FirstBootGate, KeyReplay, Restarter};
use crate::protocol::BridgeEvent;
use crate::sync_util::lock;
use log::{error, info, warn};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub struct ButtonPolicy {
    sink: Arc<dyn EventSink>,
    replay: Box<dyn KeyReplay>,
    restarter: Arc<dyn Restarter>,
    clock: Arc<dyn Clock>,
    unit: String,
    /// Shared with the per-press timer threads. `LongPress` hands out a
    /// generation per press, so a stale timer (released or superseded) is a
    /// silent no-op.
    key_m_press: Arc<Mutex<LongPress>>,
    gate: Arc<dyn FirstBootGate>,
    /// The current M press (down edge to release) was consumed by the gate.
    m_consumed: bool,
    /// Same for the knob push.
    enter_consumed: bool,
}

/// A gate that never claims a key: the behaviour without a first-boot screen.
pub struct NoGate;

impl FirstBootGate for NoGate {
    fn active(&self) -> bool {
        false
    }

    fn on_m_down(&self) {}
}

impl ButtonPolicy {
    pub fn new(
        sink: Arc<dyn EventSink>,
        replay: Box<dyn KeyReplay>,
        restarter: Arc<dyn Restarter>,
        clock: Arc<dyn Clock>,
        restart_unit: String,
        long_press: Duration,
        gate: Arc<dyn FirstBootGate>,
    ) -> ButtonPolicy {
        ButtonPolicy {
            sink,
            replay,
            restarter,
            clock,
            unit: restart_unit,
            key_m_press: Arc::new(Mutex::new(LongPress::new(long_press))),
            gate,
            m_consumed: false,
            enter_consumed: false,
        }
    }

    /// Handles one KEY event. Returns the timer thread's handle when a KEY_M
    /// press started one (production ignores it; tests join it).
    pub fn handle(&mut self, code: u16, value: i32) -> Option<JoinHandle<()>> {
        // Always broadcast the raw code/value, unchanged: grabbing the device
        // only affects other readers, not the fd already held open.
        self.sink.emit(BridgeEvent::Button { code, value });

        // The gate owns M and the knob push while the first-boot screen is up.
        // Only the down edge asks it; repeats and the release follow that
        // answer, so nothing is replayed without its press or the other way.
        if code == KEY_M {
            if value == 1 {
                self.m_consumed = self.gate.active();
                if self.m_consumed {
                    self.gate.on_m_down();
                }
            }
            if self.m_consumed {
                if value == 0 {
                    self.m_consumed = false;
                }
                return None;
            }
        } else if code == KEY_ENTER {
            if value == 1 {
                self.gate.on_knob_press();
                self.enter_consumed = self.gate.active();
            }
            if self.enter_consumed {
                if value == 0 {
                    self.enter_consumed = false;
                }
                return None;
            }
        }

        if is_replayed(code) {
            if let Err(e) = self.replay.replay(code, value) {
                error!("uinput emit failed for code={code}: {e}");
            }
        }

        let mut timer = None;
        if code == KEY_M {
            if value == 1 {
                timer = Some(self.start_timer());
            } else if value == 0 {
                // Released before the timer fired (or after it already
                // claimed the press): cancel.
                lock(&self.key_m_press).on_release();
            }
        }

        // A loud signal if the device ever reports outside the confirmed map.
        if !is_known(code) {
            warn!("unexpected key code on event0: {code}");
        }
        timer
    }

    /// Fires the restart the moment the hold crosses the threshold while the
    /// key is still held, not on release. The delay can't block the event
    /// loop (it must keep running to see the release), so a short-lived timer
    /// thread waits for the deadline and re-checks it is still this press.
    fn start_timer(&self) -> JoinHandle<()> {
        let (generation, deadline) = lock(&self.key_m_press).on_press(self.clock.now());
        let press = self.key_m_press.clone();
        let unit = self.unit.clone();
        let restarter = self.restarter.clone();
        let clock = self.clock.clone();
        thread::spawn(move || {
            clock.sleep_until(deadline);
            // should_fire claims the press, so a subsequent release does not
            // also act on it.
            if lock(&press).should_fire(clock.now(), generation) {
                info!("KEY_M long-press threshold reached while held: restarting {unit}");
                if let Err(e) = restarter.restart(&unit) {
                    error!("{e}");
                }
            }
        })
    }
}

/// Production clock.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> std::time::Instant {
        std::time::Instant::now()
    }
    fn sleep_until(&self, deadline: std::time::Instant) {
        thread::sleep(deadline.saturating_duration_since(std::time::Instant::now()));
    }
}
