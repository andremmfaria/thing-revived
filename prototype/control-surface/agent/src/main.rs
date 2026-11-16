//! superbird-bridge: hardware-input-to-WebSocket bridge, plus exclusive
//! grab + selective uinput passthrough for the 6 physical keys on
//! /dev/input/event0 (buttons 1-4, M, ESC) and knob-push/Enter, and the
//! KEY_M long-press "unwedge" hard-recovery control (systemctl restart of
//! terminal-agent). See docs/research/button-knob-clock-design.md
//! sections 1-2 and 7, and docs/research/settings-menu-feasibility.md,
//! for the design this implements.
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
//! they only exist as the WS 'button' message from here on. KEY_M's hold
//! duration is tracked separately from its (unconditional) keystroke
//! replay: a long-press-and-release of KEY_M triggers
//! `systemctl restart <terminal-agent unit>` locally, with no dependency
//! on the browser or terminal/agent's own health. KEY_ESC no longer
//! triggers this hard-recovery action — it still broadcasts its raw
//! press/release over WS unchanged, but is otherwise a no-op in this
//! agent, reserved for a future settings-menu open/close toggle (see
//! docs/research/settings-menu-feasibility.md; that menu UI is not yet
//! built).
//!
//! event1 (rotary knob, REL_HWHEEL) is left ungrabbed — nothing in
//! Chromium's stack currently double-counts it, confirmed in the design
//! doc; no uinput device needed for it either.

use evdev::uinput::{VirtualDevice, VirtualDeviceBuilder};
use evdev::{AttributeSet, Device, EventType, InputEvent, Key};
use serde::{Deserialize, Serialize};
use std::net::{TcpListener, TcpStream};
use std::process::Command;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tungstenite::protocol::{Role, WebSocket};
use tungstenite::{accept, Message};

mod settings;
mod backlight;
mod timezone;
mod bluetooth;

use settings::{load_settings, save_settings, Settings};

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

/// Minimum hold duration for KEY_M to count as a "long press" and trigger
/// hard-recovery. 1500ms is the standard long-press convention (long enough
/// that it's never hit by a normal 'm' keystroke or key-repeat, short enough
/// not to feel broken when deliberately holding it for recovery).
const KEY_M_LONG_PRESS: Duration = Duration::from_millis(1500);

/// Wire protocol for the settings-menu backend extension
/// (docs/research/settings-menu-feasibility.md §4/§5/§7/§9). This is the
/// contract the frontend menu UI is built against - field shapes here are
/// the source of truth, not any example quoted in the design doc.
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

    // --- Bluetooth (§4) ---
    /// Current (on connect) or changed (after `bt_set_power`) adapter
    /// power state.
    #[serde(rename = "bt_power")]
    BtPower { powered: bool },
    /// A `bt_set_power`/`bt_pair`/`bt_scan_start` request that failed -
    /// generic enough to cover any BT D-Bus call's failure, `context`
    /// names which request it was.
    #[serde(rename = "bt_error")]
    BtError { context: String, message: String },
    /// Scan started; bounded, see `bluetooth::SCAN_DURATION` (120-180s
    /// convention per the design doc, NOT unbounded discoverable).
    #[serde(rename = "bt_scan_started")]
    BtScanStarted { duration_secs: u64 },
    /// Scan window ended (timer expiry or explicit `bt_scan_stop`).
    #[serde(rename = "bt_scan_stopped")]
    BtScanStopped {},
    /// One newly-seen device during an active scan window. `name` is
    /// `None` when BlueZ hasn't resolved a name for that address yet.
    #[serde(rename = "bt_device_found")]
    BtDeviceFound { address: String, name: Option<String> },
    /// BlueZ's agent needs a yes/no on a displayed passkey
    /// (`RequestConfirmation`). Reply with `bt_pairing_confirm`.
    #[serde(rename = "bt_pairing_confirm_request")]
    BtPairingConfirmRequest { address: String, passkey: u32 },
    /// Notify-only passkey display (`DisplayPasskey`) - no reply expected;
    /// the far side's own keystrokes are what confirms this one.
    #[serde(rename = "bt_pairing_passkey_display")]
    BtPairingPasskeyDisplay { address: String, passkey: u32 },
    /// Final outcome of a `bt_pair` request.
    #[serde(rename = "bt_pair_result")]
    BtPairResult { address: String, success: bool, error: Option<String> },

    // --- Brightness (§5) ---
    /// Current or changed brightness. `value`/`max` are the RAW sysfs
    /// range (0..=160 on this device), not normalized to 0-100 - see
    /// backlight.rs's doc comment for why.
    #[serde(rename = "brightness")]
    Brightness { value: u32, max: u32 },

    // --- Timezone (§7) ---
    /// Current or changed timezone (IANA name, e.g. "Europe/Lisbon").
    #[serde(rename = "timezone")]
    Timezone { value: String },
    /// The full valid `timedatectl list-timezones` set, for a picker UI.
    #[serde(rename = "timezone_list")]
    TimezoneList { zones: Vec<String> },

    /// A `brightness_set`/`timezone_set` request that failed validation
    /// or the underlying syscall/subprocess - kept separate from
    /// `bt_error` so a client can distinguish categories without string
    /// matching `context`.
    #[serde(rename = "settings_error")]
    SettingsError { context: String, message: String },
}

/// Inbound commands a WS client may send. Anything not matching a known
/// `type`/shape is ignored (same permissive-ignore convention
/// terminal/agent's own `dispatch()` already uses for its protocol).
#[derive(Deserialize, Clone, Debug)]
#[serde(tag = "type")]
enum ClientCommand {
    #[serde(rename = "bt_power_get")]
    BtPowerGet {},
    #[serde(rename = "bt_set_power")]
    BtSetPower { powered: bool },
    #[serde(rename = "bt_scan_start")]
    BtScanStart {},
    #[serde(rename = "bt_scan_stop")]
    BtScanStop {},
    #[serde(rename = "bt_pair")]
    BtPair { address: String },
    #[serde(rename = "bt_pairing_confirm")]
    BtPairingConfirm { accept: bool },
    #[serde(rename = "brightness_get")]
    BrightnessGet {},
    #[serde(rename = "brightness_set")]
    BrightnessSet { value: u32 },
    #[serde(rename = "timezone_get")]
    TimezoneGet {},
    #[serde(rename = "timezone_set")]
    TimezoneSet { value: String },
    #[serde(rename = "timezone_list_get")]
    TimezoneListGet {},
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

/// Shared state the settings-menu backend needs, independent of the
/// buttons/knob plumbing above (which stays untouched). `bt` is `None`
/// when BlueZ/D-Bus wasn't reachable at startup - BT commands then
/// degrade to a `bt_error` reply per request rather than taking down the
/// rest of the daemon (buttons/knob/brightness/timezone keep working).
struct AppState {
    clients: Clients,
    bt: Option<Arc<bluetooth::BtHandle>>,
    bt_events: Sender<bluetooth::AgentEvent>,
    settings: Arc<Mutex<Settings>>,
    timezones: Arc<Vec<String>>,
}

/// Converts one `bluetooth::AgentEvent` (D-Bus-callback-originated, no WS
/// knowledge) into the matching WS broadcast, and - for `PairResult` -
/// persists nothing itself (BT power/pairing state isn't in `settings.json`,
/// bonded-device storage is already BlueZ's own job per the design doc §8.1).
fn relay_bt_events(clients: Clients, rx: std::sync::mpsc::Receiver<bluetooth::AgentEvent>) {
    use bluetooth::AgentEvent;
    for event in rx {
        let msg = match event {
            AgentEvent::ConfirmRequest { address, passkey } => {
                BridgeEvent::BtPairingConfirmRequest { address, passkey }
            }
            AgentEvent::DisplayPasskey { address, passkey } => {
                BridgeEvent::BtPairingPasskeyDisplay { address, passkey }
            }
            AgentEvent::PairResult { address, success, error } => {
                BridgeEvent::BtPairResult { address, success, error }
            }
            AgentEvent::ScanStarted { duration_secs } => BridgeEvent::BtScanStarted { duration_secs },
            AgentEvent::ScanStopped => BridgeEvent::BtScanStopped {},
            AgentEvent::DeviceFound(d) => BridgeEvent::BtDeviceFound { address: d.address, name: d.name },
        };
        broadcast(&clients, &msg);
    }
}

/// Applies one inbound `ClientCommand`, replying/broadcasting over the
/// existing WS channel. Every branch is self-contained and never panics
/// on bad/out-of-range input - a malformed or hostile WS message must
/// degrade to an error reply, not take the daemon down.
fn handle_client_command(cmd: ClientCommand, state: &AppState) {
    match cmd {
        ClientCommand::BtPowerGet {} => match &state.bt {
            Some(bt) => match bluetooth::get_powered(bt) {
                Ok(powered) => broadcast(&state.clients, &BridgeEvent::BtPower { powered }),
                Err(message) => broadcast(&state.clients, &BridgeEvent::BtError { context: "bt_power_get".into(), message }),
            },
            None => broadcast(&state.clients, &BridgeEvent::BtError { context: "bt_power_get".into(), message: "Bluetooth unavailable".into() }),
        },
        ClientCommand::BtSetPower { powered } => match &state.bt {
            Some(bt) => match bluetooth::set_powered(bt, powered) {
                Ok(()) => broadcast(&state.clients, &BridgeEvent::BtPower { powered }),
                Err(message) => broadcast(&state.clients, &BridgeEvent::BtError { context: "bt_set_power".into(), message }),
            },
            None => broadcast(&state.clients, &BridgeEvent::BtError { context: "bt_set_power".into(), message: "Bluetooth unavailable".into() }),
        },
        ClientCommand::BtScanStart {} => match &state.bt {
            Some(bt) => {
                if let Err(message) = bluetooth::start_scan(bt, state.bt_events.clone()) {
                    broadcast(&state.clients, &BridgeEvent::BtError { context: "bt_scan_start".into(), message });
                }
            }
            None => broadcast(&state.clients, &BridgeEvent::BtError { context: "bt_scan_start".into(), message: "Bluetooth unavailable".into() }),
        },
        ClientCommand::BtScanStop {} => match &state.bt {
            Some(bt) => {
                if let Err(message) = bluetooth::stop_scan(bt) {
                    broadcast(&state.clients, &BridgeEvent::BtError { context: "bt_scan_stop".into(), message });
                }
            }
            None => broadcast(&state.clients, &BridgeEvent::BtError { context: "bt_scan_stop".into(), message: "Bluetooth unavailable".into() }),
        },
        ClientCommand::BtPair { address } => match &state.bt {
            Some(bt) => {
                // Device1.Pair() blocks on this process's own agent
                // callback getting its WS round-trip answered - must run
                // off the WS-read thread, see bluetooth::pair's doc comment.
                let bt = bt.clone();
                let events = state.bt_events.clone();
                thread::spawn(move || bluetooth::pair(&bt, &address, &events));
            }
            None => broadcast(&state.clients, &BridgeEvent::BtError { context: "bt_pair".into(), message: "Bluetooth unavailable".into() }),
        },
        ClientCommand::BtPairingConfirm { accept } => match &state.bt {
            Some(bt) => bluetooth::submit_confirm(bt, accept),
            None => eprintln!("[bridge] bt_pairing_confirm received but Bluetooth is unavailable"),
        },
        ClientCommand::BrightnessGet {} => send_brightness(state),
        ClientCommand::BrightnessSet { value } => {
            match backlight::set(value) {
                Ok(()) => {
                    let mut settings = state.settings.lock().unwrap();
                    settings.brightness = Some(value);
                    save_settings(&settings);
                    drop(settings);
                    send_brightness(state);
                }
                Err(message) => broadcast(&state.clients, &BridgeEvent::SettingsError { context: "brightness_set".into(), message }),
            }
        }
        ClientCommand::TimezoneGet {} => send_timezone(state),
        ClientCommand::TimezoneSet { value } => {
            match timezone::set_timezone(&value, &state.timezones) {
                Ok(()) => {
                    let mut settings = state.settings.lock().unwrap();
                    settings.timezone = Some(value);
                    save_settings(&settings);
                    drop(settings);
                    send_timezone(state);
                }
                Err(message) => broadcast(&state.clients, &BridgeEvent::SettingsError { context: "timezone_set".into(), message }),
            }
        }
        ClientCommand::TimezoneListGet {} => {
            broadcast(&state.clients, &BridgeEvent::TimezoneList { zones: (*state.timezones).clone() });
        }
    }
}

fn send_brightness(state: &AppState) {
    match (backlight::read_current(), backlight::read_max()) {
        (Ok(value), Ok(max)) => broadcast(&state.clients, &BridgeEvent::Brightness { value, max }),
        (res1, res2) => {
            let message = res1.and(res2).unwrap_err();
            broadcast(&state.clients, &BridgeEvent::SettingsError { context: "brightness_get".into(), message });
        }
    }
}

fn send_timezone(state: &AppState) {
    match timezone::get_timezone() {
        Ok(value) => broadcast(&state.clients, &BridgeEvent::Timezone { value }),
        Err(message) => broadcast(&state.clients, &BridgeEvent::SettingsError { context: "timezone_get".into(), message }),
    }
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
    // Tracks when the current KEY_M press started, so release can measure
    // hold duration. Only KEY_M needs this - no general per-key map.
    let mut key_m_pressed_at: Option<Instant> = None;
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

                        // KEY_M long-press hard-recovery: restart terminal-agent
                        // directly from here, no WS round-trip, no Chromium/
                        // terminal-agent dependency on the critical path. Track the
                        // press instant on value==1, measure elapsed on value==0
                        // (release), and only act if the hold met the long-press
                        // threshold - a normal tap/repeat of 'm' does nothing extra
                        // beyond its existing uinput passthrough above.
                        if code == KEY_M {
                            if value == 1 {
                                key_m_pressed_at = Some(Instant::now());
                            } else if value == 0 {
                                if let Some(pressed_at) = key_m_pressed_at.take() {
                                    if pressed_at.elapsed() >= KEY_M_LONG_PRESS {
                                        println!(
                                            "[bridge] KEY_M long-press released: restarting {TERMINAL_AGENT_UNIT}"
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
                                }
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

/// One OS thread pair per client connection, same split-read/write-socket
/// pattern terminal/agent's own `handle_client` already uses (see that
/// file's doc comment for the full rationale): a `try_clone()`'d second
/// `TcpStream` backs an independent writer `WebSocket`, so broadcasts
/// (buttons/knob events, BT/brightness/timezone replies) never block on,
/// or get blocked by, this client's own inbound command reads - needed
/// here specifically because `ClientCommand::BtPair` can take a while
/// (Device1.Pair() blocking on its own agent round-trip) without ever
/// touching this function directly (it's spawned onto its own thread by
/// `handle_client_command`), but a slow/idle client must still never
/// stall buttons/knob delivery to every OTHER connected client either.
fn handle_client(stream: TcpStream, state: Arc<AppState>) {
    let write_stream = match stream.try_clone() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[bridge] failed to clone client socket: {e}");
            return;
        }
    };
    let mut ws_read = match accept(stream) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[bridge] websocket handshake failed: {e}");
            return;
        }
    };
    let mut ws_write = WebSocket::from_raw_socket(write_stream, Role::Server, None);

    let (tx, rx) = channel::<String>();
    state.clients.lock().unwrap().push(tx);
    println!("[bridge] ui client connected");

    thread::spawn(move || loop {
        match rx.recv_timeout(Duration::from_secs(10)) {
            Ok(json) => {
                if ws_write.send(Message::Text(json.into())).is_err() {
                    break;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // Idle ping so dead connections get noticed promptly.
                if ws_write.send(Message::Ping(Vec::new().into())).is_err() {
                    break;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    });

    loop {
        match ws_read.read() {
            Ok(Message::Text(text)) => {
                match serde_json::from_str::<ClientCommand>(&text) {
                    Ok(cmd) => handle_client_command(cmd, &state),
                    Err(e) => eprintln!("[bridge] ignoring unrecognized client message: {e}"),
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => continue, // binary/ping/pong - not part of this protocol
            Err(_) => break,
        }
    }
    println!("[bridge] ui client disconnected");
    // The writer thread's channel sender still lives in `state.clients`
    // (same lazy-prune-on-send-failure pattern as before); it notices the
    // closed socket on its own next write attempt and exits.
}

fn main() {
    let clients: Clients = Arc::new(Mutex::new(Vec::new()));

    let uinput_dev = build_uinput_passthrough();
    println!("[bridge] uinput passthrough device ready (KEY_M, KEY_ENTER only)");

    let buttons_clients = clients.clone();
    thread::spawn(move || run_buttons("/dev/input/event0", buttons_clients, uinput_dev));

    let knob_clients = clients.clone();
    thread::spawn(move || run_knob("/dev/input/event1", knob_clients));

    // --- Settings-menu backend (docs/research/settings-menu-feasibility.md) ---

    let settings = load_settings();
    println!("[bridge] settings: loaded {settings:?} from {}", settings::SETTINGS_PATH);

    // Brightness persistence (§8.1): systemd-backlight@backlight:backlight.service
    // IS present and active on this image, but its saved-state directory
    // (/var/lib/systemd/backlight/) was found empty on a live check - its
    // "save" half is unconfirmed to actually fire on this BSP's
    // reboot/poweroff path, so this agent re-applies its own persisted
    // value at startup too (belt-and-suspenders, not a replacement).
    if let Some(value) = settings.brightness {
        match backlight::set(value) {
            Ok(()) => println!("[bridge] brightness: re-applied saved value {value} at startup"),
            Err(e) => eprintln!("[bridge] brightness: failed to re-apply saved value {value} at startup: {e}"),
        }
    }

    // Timezone persistence (§7.2): `timedatectl set-timezone`'s own
    // /etc/localtime symlink write does not survive a reboot on this
    // read-only-rootfs image, so re-apply unconditionally at every startup.
    let timezones = Arc::new(timezone::list_timezones());
    println!("[bridge] timezone: {} valid IANA zones cached at startup", timezones.len());
    if let Some(ref tz) = settings.timezone {
        match timezone::set_timezone(tz, &timezones) {
            Ok(()) => println!("[bridge] timezone: re-applied saved value {tz} at startup"),
            Err(e) => eprintln!("[bridge] timezone: failed to re-apply saved value {tz} at startup: {e}"),
        }
    }

    let (bt_events_tx, bt_events_rx) = channel::<bluetooth::AgentEvent>();
    {
        let clients = clients.clone();
        thread::spawn(move || relay_bt_events(clients, bt_events_rx));
    }
    let bt = match bluetooth::connect_and_register_agent(bt_events_tx.clone()) {
        Ok(handle) => {
            println!("[bridge] bt: connected to system bus, agent registered");
            Some(Arc::new(handle))
        }
        Err(e) => {
            eprintln!(
                "[bridge] bt: D-Bus connection/agent registration failed: {e} -- Bluetooth \
                 commands will return bt_error until this is resolved; buttons/knob/\
                 brightness/timezone are unaffected"
            );
            None
        }
    };

    let state = Arc::new(AppState {
        clients: clients.clone(),
        bt,
        bt_events: bt_events_tx,
        settings: Arc::new(Mutex::new(settings)),
        timezones,
    });

    let listener = TcpListener::bind("0.0.0.0:8765").expect("bind 0.0.0.0:8765 failed");
    println!("[bridge] WebSocket server listening on ws://0.0.0.0:8765");

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let state = state.clone();
                thread::spawn(move || handle_client(stream, state));
            }
            Err(e) => eprintln!("[bridge] accept error: {e}"),
        }
    }
}
