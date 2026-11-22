//! evdev readers for the buttons device (grabbed exclusively, EVIOCGRAB) and
//! the knob device (left ungrabbed). Both loops never return.

use crate::ports::EventSink;
use crate::protocol::BridgeEvent;
use crate::services::input::ButtonPolicy;
use crate::utils::config::InputConfig;
use evdev::{Device, EventType};
use log::{error, info, warn};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// Opens an evdev device by path, retrying for a few seconds - the device nodes exist
/// at boot, but this keeps the daemon resilient if it's started slightly early or the
/// node is briefly busy.
pub fn open_device(path: &str, cfg: &InputConfig) -> Device {
    for attempt in 1..=cfg.open_retries {
        match Device::open(path) {
            Ok(d) => return d,
            Err(e) => {
                warn!("open({path}) attempt {attempt} failed: {e}, retrying...");
                thread::sleep(Duration::from_millis(cfg.open_retry_delay_ms));
            }
        }
    }
    panic!("could not open {path} after {} attempts", cfg.open_retries);
}

pub fn run_buttons(cfg: InputConfig, mut policy: ButtonPolicy) {
    let path = cfg.buttons_device.as_str();
    let mut device = open_device(path, &cfg);
    match device.grab() {
        Ok(()) => info!("buttons: grabbed {path} exclusively (EVIOCGRAB)"),
        Err(e) => error!(
            "buttons: FAILED to grab {path}: {e} -- buttons 1-4/ESC will leak \
             raw keydowns to the browser until this succeeds"
        ),
    }
    info!("buttons: listening on {path} ({})", device.name().unwrap_or("?"));
    loop {
        match device.fetch_events() {
            Ok(events) => {
                for ev in events {
                    if ev.event_type() == EventType::KEY {
                        policy.handle(ev.code(), ev.value());
                    }
                }
            }
            Err(e) => {
                error!("buttons: fetch_events error: {e}");
                thread::sleep(Duration::from_millis(cfg.read_error_delay_ms));
            }
        }
    }
}

pub fn run_knob(cfg: InputConfig, sink: Arc<dyn EventSink>) {
    let path = cfg.knob_device.as_str();
    let mut device = open_device(path, &cfg);
    info!("knob: listening on {path} ({})", device.name().unwrap_or("?"));
    loop {
        match device.fetch_events() {
            Ok(events) => {
                for ev in events {
                    // REL_HWHEEL per hardware-verification.md; pass through whatever
                    // RELATIVE-type code arrives, raw, rather than assuming the exact
                    // sub-code number.
                    if ev.event_type() == EventType::RELATIVE {
                        sink.emit(BridgeEvent::Knob { delta: ev.value() });
                    }
                }
            }
            Err(e) => {
                error!("knob: fetch_events error: {e}");
                thread::sleep(Duration::from_millis(cfg.read_error_delay_ms));
            }
        }
    }
}
