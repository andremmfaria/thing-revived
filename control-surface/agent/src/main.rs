//! superbird-bridge: v0 hardware-input-to-WebSocket proof-of-life.
//!
//! Reads raw evdev events from the Car Thing's gpio-keys (buttons, /dev/input/event0)
//! and rotary knob (/dev/input/event1), and broadcasts them as small JSON messages to
//! any WebSocket client connected on ws://0.0.0.0:8765.
//!
//! Deliberately generic: button events carry the RAW Linux key code, not a
//! human-readable button name. The exact code->physical-button mapping has not been
//! confirmed by a live button press at the time this was written (see project docs /
//! final report) - hardcoding names here would be guesswork. Once a human has pressed
//! each button while watching this bridge's output (or `ui/`'s display), the real
//! mapping can be captured from the printed/displayed codes and only THEN turned into
//! a translation table, as a deliberate follow-up change - not bundled into this file.

use evdev::{Device, EventType};
use serde::Serialize;
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tungstenite::{accept, Message};

#[derive(Serialize, Clone, Debug)]
#[serde(tag = "type")]
enum BridgeEvent {
    /// A physical button's raw evdev KEY_* code. `value`: 1 = pressed, 0 = released,
    /// 2 = autorepeat (evdev convention) - passed through unmodified, raw.
    #[serde(rename = "button")]
    Button { code: u16, value: i32 },
    /// A knob rotation step, raw REL_HWHEEL tick value.
    #[serde(rename = "knob")]
    Knob { delta: i32 },
}

type Clients = Arc<Mutex<Vec<Sender<String>>>>;

fn broadcast(clients: &Clients, msg: &BridgeEvent) {
    let json = match serde_json::to_string(msg) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("[bridge] failed to serialize event: {e}");
            return;
        }
    };
    println!("[bridge] {json}");
    let mut guard = clients.lock().unwrap();
    guard.retain(|tx| tx.send(json.clone()).is_ok());
}

/// Opens an evdev device by path, retrying for a few seconds - the device nodes exist
/// at boot, but this keeps the daemon resilient if it's started slightly early or the
/// node is briefly busy.
fn open_device(path: &str) -> Device {
    for attempt in 1..=10 {
        match Device::open(path) {
            Ok(d) => return d,
            Err(e) => {
                eprintln!("[bridge] open({path}) attempt {attempt} failed: {e}, retrying...");
                thread::sleep(Duration::from_millis(500));
            }
        }
    }
    panic!("[bridge] could not open {path} after 10 attempts");
}

fn run_buttons(path: &str, clients: Clients) {
    let mut device = open_device(path);
    println!(
        "[bridge] buttons: listening on {path} ({})",
        device.name().unwrap_or("?")
    );
    loop {
        match device.fetch_events() {
            Ok(events) => {
                for ev in events {
                    if ev.event_type() == EventType::KEY {
                        broadcast(
                            &clients,
                            &BridgeEvent::Button {
                                code: ev.code(),
                                value: ev.value(),
                            },
                        );
                    }
                }
            }
            Err(e) => {
                eprintln!("[bridge] buttons: fetch_events error: {e}");
                thread::sleep(Duration::from_millis(200));
            }
        }
    }
}

fn run_knob(path: &str, clients: Clients) {
    let mut device = open_device(path);
    println!(
        "[bridge] knob: listening on {path} ({})",
        device.name().unwrap_or("?")
    );
    loop {
        match device.fetch_events() {
            Ok(events) => {
                for ev in events {
                    // REL_HWHEEL per hardware-verification.md; pass through whatever
                    // RELATIVE-type code arrives, raw, rather than assuming the exact
                    // sub-code number.
                    if ev.event_type() == EventType::RELATIVE {
                        broadcast(&clients, &BridgeEvent::Knob { delta: ev.value() });
                    }
                }
            }
            Err(e) => {
                eprintln!("[bridge] knob: fetch_events error: {e}");
                thread::sleep(Duration::from_millis(200));
            }
        }
    }
}

fn handle_client(stream: TcpStream, clients: Clients) {
    let mut socket = match accept(stream) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[bridge] websocket handshake failed: {e}");
            return;
        }
    };
    let (tx, rx) = channel::<String>();
    clients.lock().unwrap().push(tx);
    println!("[bridge] ui client connected");

    loop {
        match rx.recv_timeout(Duration::from_secs(10)) {
            Ok(json) => {
                if socket.send(Message::Text(json.into())).is_err() {
                    break;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // Idle ping so dead connections get noticed promptly.
                if socket.send(Message::Ping(Vec::new().into())).is_err() {
                    break;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    println!("[bridge] ui client disconnected");
}

fn main() {
    let clients: Clients = Arc::new(Mutex::new(Vec::new()));

    let buttons_clients = clients.clone();
    thread::spawn(move || run_buttons("/dev/input/event0", buttons_clients));

    let knob_clients = clients.clone();
    thread::spawn(move || run_knob("/dev/input/event1", knob_clients));

    let listener = TcpListener::bind("0.0.0.0:8765").expect("bind 0.0.0.0:8765 failed");
    println!("[bridge] WebSocket server listening on ws://0.0.0.0:8765");

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let clients = clients.clone();
                thread::spawn(move || handle_client(stream, clients));
            }
            Err(e) => eprintln!("[bridge] accept error: {e}"),
        }
    }
}
