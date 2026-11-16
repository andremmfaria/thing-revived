//! superbird-bridge: hardware-input-to-WebSocket bridge, plus exclusive
//! grab + selective uinput passthrough for the 6 physical keys on
//! /dev/input/event0 (buttons 1-4, M, ESC) and knob-push/Enter, and the
//! KEY_ESC "unwedge" hard-recovery control (systemctl restart of
//! terminal-agent). See docs/research/button-knob-clock-design.md
//! sections 1-2 and 7 for the design this implements.
//!
//! Reads raw evdev events from the Car Thing's gpio-keys (buttons,
//! /dev/input/event0) and rotary knob (/dev/input/event1), and broadcasts
//! them as small JSON messages to any WebSocket client connected on
//! ws://0.0.0.0:8765.
//!
//! Full confirmed key map on event0: KEY_ESC=1, KEY_1..KEY_4=2..5,
//! KEY_ENTER=28, KEY_M=50.
//!
//! event0 is grabbed exclusively (EVIOCGRAB) immediately after opening, so
//! none of these 6 keys reach Chromium's own input stack as a DOM keydown
//! anymore. KEY_M and KEY_ENTER are selectively re-injected through a
//! uinput virtual keyboard so they keep behaving like ordinary keystrokes
//! in the terminal frontend; KEY_1..KEY_4 and KEY_ESC are NOT replayed —
//! they only exist as the WS 'button' message from here on. KEY_ESC
//! additionally triggers `systemctl restart <terminal-agent unit>`
//! locally, with no dependency on the browser or terminal/agent's own
//! health (see module doc above).
//!
//! event1 (rotary knob, REL_HWHEEL) is left ungrabbed — nothing in
//! Chromium's stack currently double-counts it, confirmed in the design
//! doc; no uinput device needed for it either.

use evdev::uinput::{VirtualDevice, VirtualDeviceBuilder};
use evdev::{AttributeSet, Device, EventType, InputEvent, Key};
use serde::Serialize;
use std::net::{TcpListener, TcpStream};
use std::process::Command;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tungstenite::{accept, Message};

/// Raw Linux KEY_* codes on /dev/input/event0, confirmed live.
const KEY_ESC: u16 = 1;
const KEY_1: u16 = 2;
const KEY_4: u16 = 5;
const KEY_ENTER: u16 = 28;
const KEY_M: u16 = 50;

/// systemd unit name for the on-device terminal agent, confirmed via
/// `systemctl show thing-revived-terminal-agent.service` on the live
/// device (matches the design doc's placeholder exactly).
const TERMINAL_AGENT_UNIT: &str = "thing-revived-terminal-agent.service";

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

/// Builds the uinput virtual keyboard used to selectively replay KEY_M and
/// KEY_ENTER once the real device (event0) is grabbed exclusively. Capability
/// set is deliberately narrow - exactly the two keys that must keep behaving
/// like ordinary keystrokes in the browser.
fn build_uinput_passthrough() -> VirtualDevice {
    let mut keys = AttributeSet::<Key>::new();
    keys.insert(Key::KEY_M);
    keys.insert(Key::KEY_ENTER);
    VirtualDeviceBuilder::new()
        .expect("uinput: /dev/uinput open failed")
        .name("thing-revived-passthrough")
        .with_keys(&keys)
        .expect("uinput: with_keys failed")
        .build()
        .expect("uinput: virtual device build failed")
}

fn run_buttons(path: &str, clients: Clients, mut uinput_dev: VirtualDevice) {
    let mut device = open_device(path);
    match device.grab() {
        Ok(()) => println!("[bridge] buttons: grabbed {path} exclusively (EVIOCGRAB)"),
        Err(e) => eprintln!(
            "[bridge] buttons: FAILED to grab {path}: {e} -- buttons 1-4/ESC will leak \
             raw keydowns to the browser until this succeeds"
        ),
    }
    println!(
        "[bridge] buttons: listening on {path} ({})",
        device.name().unwrap_or("?")
    );
    loop {
        match device.fetch_events() {
            Ok(events) => {
                for ev in events {
                    if ev.event_type() == EventType::KEY {
                        let code = ev.code();
                        let value = ev.value();

                        // Always broadcast the raw code/value over WS, unchanged -
                        // grabbing the device only affects *other* readers, not the
                        // fd we already hold open here.
                        broadcast(&clients, &BridgeEvent::Button { code, value });

                        // Selective uinput replay: only M and Enter keep acting like
                        // ordinary keystrokes in the browser. 1-4 and ESC are not
                        // replayed anywhere - they exist only as the WS message now.
                        if code == KEY_M || code == KEY_ENTER {
                            let out_ev = InputEvent::new(EventType::KEY, code, value);
                            if let Err(e) = uinput_dev.emit(&[out_ev]) {
                                eprintln!("[bridge] uinput emit failed for code={code}: {e}");
                            }
                        }

                        // KEY_ESC hard-recovery: restart terminal-agent directly from
                        // here, no WS round-trip, no Chromium/terminal-agent
                        // dependency on the critical path. Only on press (value==1),
                        // not release/repeat.
                        if code == KEY_ESC && value == 1 {
                            println!(
                                "[bridge] KEY_ESC pressed: restarting {TERMINAL_AGENT_UNIT}"
                            );
                            match Command::new("systemctl")
                                .args(["restart", TERMINAL_AGENT_UNIT])
                                .spawn()
                            {
                                Ok(_) => {}
                                Err(e) => eprintln!(
                                    "[bridge] failed to spawn systemctl restart {TERMINAL_AGENT_UNIT}: {e}"
                                ),
                            }
                        }

                        // Sanity check for the hardware range this bridge was built
                        // against (buttons 1-4 = codes 2..5) - not enforced, just a
                        // loud signal if the device ever reports something outside
                        // the documented/confirmed map.
                        if !(KEY_1..=KEY_4).contains(&code)
                            && code != KEY_ESC
                            && code != KEY_M
                            && code != KEY_ENTER
                        {
                            eprintln!("[bridge] unexpected key code on event0: {code}");
                        }
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

    let uinput_dev = build_uinput_passthrough();
    println!("[bridge] uinput passthrough device ready (KEY_M, KEY_ENTER only)");

    let buttons_clients = clients.clone();
    thread::spawn(move || run_buttons("/dev/input/event0", buttons_clients, uinput_dev));

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
