//! Button policy for the grabbed buttons device. Every key event is
//! broadcast unchanged; M and Enter are additionally replayed; M also feeds
//! the long-press detector, whose firing restarts a systemd unit directly
//! (no WebSocket round trip, no dependency on the browser or terminal agent).

use crate::domain::keys::{is_known, is_replayed, KEY_M};
use crate::domain::longpress::LongPress;
use crate::ports::{Clock, EventSink, KeyReplay, Restarter};
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
}

impl ButtonPolicy {
    pub fn new(
        sink: Arc<dyn EventSink>,
        replay: Box<dyn KeyReplay>,
        restarter: Arc<dyn Restarter>,
        clock: Arc<dyn Clock>,
        restart_unit: String,
        long_press: Duration,
    ) -> ButtonPolicy {
        ButtonPolicy { sink, replay, restarter, clock, unit: restart_unit, key_m_press: Arc::new(Mutex::new(LongPress::new(long_press))) }
    }

    /// Handles one KEY event. Returns the timer thread's handle when a KEY_M
    /// press started one (production ignores it; tests join it).
    pub fn handle(&mut self, code: u16, value: i32) -> Option<JoinHandle<()>> {
        // Always broadcast the raw code/value, unchanged: grabbing the device
        // only affects other readers, not the fd already held open.
        self.sink.emit(BridgeEvent::Button { code, value });

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
