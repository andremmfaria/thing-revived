//! controller-agent: hardware-input-to-WebSocket bridge, plus exclusive
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
//! ws://127.0.0.1:8765 (loopback only; see `bind_addr`).
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

use log::{debug, error, info, trace, warn};
use utils::config::{Config, InputConfig, ServerConfig};
use evdev::uinput::{VirtualDevice, VirtualDeviceBuilder};
use evdev::{AttributeSet, Device, EventType, InputEvent, Key};
use app::error::AppError;
use protocol::{BridgeEvent, ClientCommand, PairedDeviceWire};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::process::Command;
use std::sync::mpsc::{channel, sync_channel, Sender, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tungstenite::protocol::{Role, WebSocket};
use tungstenite::{accept, Message};

pub mod domain;
pub mod sync_util;
pub mod settings;
pub mod backlight;
pub mod timezone;
pub mod bluetooth;
pub mod wifi;
pub mod utils;
pub mod protocol;
pub mod app;

use settings::{load_settings, save_settings, Settings};
use domain::longpress::LongPress;
use sync_util::lock;

/// Raw Linux KEY_* codes on /dev/input/event0, confirmed live.
const KEY_ESC: u16 = 1;
const KEY_1: u16 = 2;
const KEY_4: u16 = 5;
const KEY_ENTER: u16 = 28;
const KEY_M: u16 = 50;

// The systemd unit restarted by the KEY_M long press is `[input]
// restart_unit` (default thing-revived-terminal-agent.service, confirmed via
// `systemctl show` on the live device).

// `[input] longpress_ms` (default 1500 ms) is the minimum hold duration for
// KEY_M to count as a "long press" and trigger hard-recovery: long enough
// that it is never hit by a normal 'm' keystroke or key-repeat, short enough
// not to feel broken when deliberately holding it for recovery.

/// Compiled-in WebSocket bind (also `[server] bind`'s default). The only
/// legitimate client is the Chromium kiosk on the device itself, and this
/// socket can change Wi-Fi, Bluetooth and system settings with no
/// authentication, so it must not be reachable from the network. From a dev
/// host use `ssh -L 8765:127.0.0.1:8765 <device>` instead of a wide bind.
const DEFAULT_BIND: &str = "127.0.0.1:8765";

/// Env var a developer can set to deliberately override `[server] bind`
/// (e.g. `CONTROLLER_BIND=0.0.0.0:8765`). A non-loopback value is honoured
/// but logged loudly in `run`. An unparsable value falls back to the
/// loopback default rather than to anything wider.
const BIND_ENV: &str = "CONTROLLER_BIND";

/// Resolves the listen address from the raw env value (`None` = unset) and
/// the configured `[server] bind`. The env var has priority over the file.
fn bind_addr(env_value: Option<&str>, configured: SocketAddr) -> SocketAddr {
    match env_value {
        None => configured,
        Some(raw) => raw.trim().parse().unwrap_or_else(|e| {
            warn!("{BIND_ENV}={raw:?} is not a valid host:port ({e}); using {DEFAULT_BIND}");
            DEFAULT_BIND.parse().expect("DEFAULT_BIND is a valid socket address")
        }),
    }
}

fn is_loopback(addr: &SocketAddr) -> bool {
    match addr.ip() {
        IpAddr::V4(ip) => ip.is_loopback(),
        IpAddr::V6(ip) => ip.is_loopback(),
    }
}

// Per-client outbound queue depth is `[server] client_queue_capacity`. A
// healthy kiosk drains it instantly; a client that falls that far behind
// (stalled socket) is disconnected instead of letting its queue grow without
// bound on a 512 MB device.

struct ClientSlot {
    tx: SyncSender<String>,
    /// A handle on the client's socket, used only to force-close it when the
    /// client is evicted. That unblocks its writer thread (stuck in a send)
    /// and its reader thread. `None` in tests that have no socket.
    stream: Option<TcpStream>,
}

type Clients = Arc<Mutex<Vec<ClientSlot>>>;

/// How much of an event goes to the journal (flash-backed on the device).
enum LogPolicy {
    /// High-frequency traffic (knob ticks, button edges, the Wi-Fi status
    /// poll): never logged.
    Silent,
    /// Type only: the payload is a pairing passkey, treated as a secret.
    TypeOnly,
    Full,
}

fn log_policy(msg: &BridgeEvent) -> LogPolicy {
    match msg {
        BridgeEvent::Knob { .. } | BridgeEvent::Button { .. } | BridgeEvent::WifiStatus { .. } => LogPolicy::Silent,
        BridgeEvent::BtPairingConfirmRequest { .. } | BridgeEvent::BtPairingPasskeyDisplay { .. } => LogPolicy::TypeOnly,
        _ => LogPolicy::Full,
    }
}

fn broadcast(clients: &Clients, msg: &BridgeEvent) {
    let json = match serde_json::to_string(msg) {
        Ok(j) => j,
        Err(e) => {
            error!("failed to serialize event: {e}");
            return;
        }
    };
    match log_policy(msg) {
        // High-frequency traffic is trace only (and carries no secrets).
        LogPolicy::Silent => trace!("{json}"),
        LogPolicy::TypeOnly => {
            let ty = serde_json::to_value(msg).ok().and_then(|v| v["type"].as_str().map(str::to_owned));
            info!("{} (payload not logged)", ty.unwrap_or_default());
        }
        LogPolicy::Full => info!("{json}"),
    }
    // try_send never blocks, so the lock is held only for O(clients) cheap
    // pushes. Evicted clients are closed after the lock is released.
    let mut evicted = Vec::new();
    {
        let mut guard = lock(clients);
        let mut i = 0;
        while i < guard.len() {
            match guard[i].tx.try_send(json.clone()) {
                Ok(()) => i += 1,
                Err(TrySendError::Full(_)) => {
                    warn!("ui client queue full, disconnecting it");
                    evicted.push(guard.swap_remove(i));
                }
                // Writer thread is gone: prune.
                Err(TrySendError::Disconnected(_)) => evicted.push(guard.swap_remove(i)),
            }
        }
    }
    for slot in evicted {
        if let Some(stream) = slot.stream {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    }
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
    wifi: Arc<wifi::Wifi>,
    server: ServerConfig,
}

/// Looks up the current paired-device set and broadcasts it (or a
/// `bt_error` if the lookup itself fails) - the one place every
/// "the paired set might have just changed" path funnels through, so
/// `bt_paired_list_get`, a successful `bt_connect`/`bt_disconnect`/
/// `bt_forget`, and a successful new pairing (`relay_bt_events` below) all
/// keep the frontend's list in sync via the exact same call.
fn broadcast_paired_list(clients: &Clients, bt: &bluetooth::BtHandle) {
    match bluetooth::list_paired_devices(bt) {
        Ok(devices) => broadcast(
            clients,
            &BridgeEvent::BtPairedList { devices: devices.into_iter().map(PairedDeviceWire::from).collect() },
        ),
        Err(AppError { message, .. }) => broadcast(clients, &BridgeEvent::BtError { context: "bt_paired_list_get".into(), message }),
    }
}

/// Converts one `bluetooth::AgentEvent` (D-Bus-callback-originated, no WS
/// knowledge) into the matching WS broadcast, and - for `PairResult` -
/// persists nothing itself (BT power/pairing state isn't in `settings.json`,
/// bonded-device storage is already BlueZ's own job per the design doc §8.1).
/// `bt` is `None` when BlueZ wasn't reachable at startup, same degrade-
/// gracefully convention as the rest of this file's BT handling - a
/// successful pair can't happen at all in that case, so there's nothing to
/// refresh, but the match arm still has to handle the possibility.
fn relay_bt_events(clients: Clients, bt: Option<Arc<bluetooth::BtHandle>>, rx: std::sync::mpsc::Receiver<bluetooth::AgentEvent>) {
    use bluetooth::AgentEvent;
    for event in rx {
        let refresh_paired_list = matches!(event, AgentEvent::PairResult { success: true, .. });
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
        // A new pairing just succeeded: the paired set changed, so push a
        // fresh bt_paired_list proactively (settings-menu-feasibility.md's
        // "no manual refresh" requirement) without waiting for the
        // frontend to ask.
        if refresh_paired_list {
            if let Some(bt) = &bt {
                broadcast_paired_list(&clients, bt);
            }
        }
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
                Err(AppError { message, .. }) => broadcast(&state.clients, &BridgeEvent::BtError { context: "bt_power_get".into(), message }),
            },
            None => broadcast(&state.clients, &BridgeEvent::BtError { context: "bt_power_get".into(), message: "Bluetooth unavailable".into() }),
        },
        ClientCommand::BtSetPower { powered } => match &state.bt {
            Some(bt) => match bluetooth::set_powered(bt, powered) {
                Ok(()) => broadcast(&state.clients, &BridgeEvent::BtPower { powered }),
                Err(AppError { message, .. }) => broadcast(&state.clients, &BridgeEvent::BtError { context: "bt_set_power".into(), message }),
            },
            None => broadcast(&state.clients, &BridgeEvent::BtError { context: "bt_set_power".into(), message: "Bluetooth unavailable".into() }),
        },
        ClientCommand::BtScanStart {} => match &state.bt {
            Some(bt) => {
                if let Err(AppError { message, .. }) = bluetooth::start_scan(bt, state.bt_events.clone()) {
                    broadcast(&state.clients, &BridgeEvent::BtError { context: "bt_scan_start".into(), message });
                }
            }
            None => broadcast(&state.clients, &BridgeEvent::BtError { context: "bt_scan_start".into(), message: "Bluetooth unavailable".into() }),
        },
        ClientCommand::BtScanStop {} => match &state.bt {
            Some(bt) => {
                if let Err(AppError { message, .. }) = bluetooth::stop_scan(bt) {
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
            None => warn!("bt_pairing_confirm received but Bluetooth is unavailable"),
        },
        ClientCommand::BtPairedListGet {} => match &state.bt {
            Some(bt) => broadcast_paired_list(&state.clients, bt),
            None => broadcast(&state.clients, &BridgeEvent::BtError { context: "bt_paired_list_get".into(), message: "Bluetooth unavailable".into() }),
        },
        ClientCommand::BtConnect { address } => match &state.bt {
            Some(bt) => {
                // Device1.Connect() can take several seconds for a real
                // reconnect (same "don't block the WS read loop" reasoning
                // as bt_pair's own thread::spawn above).
                let bt = bt.clone();
                let clients = state.clients.clone();
                thread::spawn(move || match bluetooth::connect_device(&bt, &address) {
                    Ok(()) => broadcast_paired_list(&clients, &bt),
                    Err(AppError { message, .. }) => broadcast(&clients, &BridgeEvent::BtError { context: "bt_connect".into(), message }),
                });
            }
            None => broadcast(&state.clients, &BridgeEvent::BtError { context: "bt_connect".into(), message: "Bluetooth unavailable".into() }),
        },
        ClientCommand::BtDisconnect { address } => match &state.bt {
            Some(bt) => {
                let bt = bt.clone();
                let clients = state.clients.clone();
                thread::spawn(move || match bluetooth::disconnect_device(&bt, &address) {
                    Ok(()) => broadcast_paired_list(&clients, &bt),
                    Err(AppError { message, .. }) => broadcast(&clients, &BridgeEvent::BtError { context: "bt_disconnect".into(), message }),
                });
            }
            None => broadcast(&state.clients, &BridgeEvent::BtError { context: "bt_disconnect".into(), message: "Bluetooth unavailable".into() }),
        },
        ClientCommand::BtForget { address } => match &state.bt {
            // RemoveDevice is a local adapter-side call (no remote
            // round-trip, unlike Connect/Disconnect) - fast enough to run
            // inline, matching bt_scan_start/bt_scan_stop's own inline
            // treatment above.
            Some(bt) => match bluetooth::forget_device(bt, &address) {
                Ok(()) => broadcast_paired_list(&state.clients, bt),
                Err(AppError { message, .. }) => broadcast(&state.clients, &BridgeEvent::BtError { context: "bt_forget".into(), message }),
            },
            None => broadcast(&state.clients, &BridgeEvent::BtError { context: "bt_forget".into(), message: "Bluetooth unavailable".into() }),
        },
        ClientCommand::BrightnessGet {} => send_brightness(state),
        ClientCommand::BrightnessSet { value } => {
            match backlight::set(value) {
                Ok(()) => {
                    let mut settings = lock(&state.settings);
                    settings.brightness = Some(value);
                    save_settings(&settings);
                    drop(settings);
                    send_brightness(state);
                }
                Err(AppError { message, .. }) => broadcast(&state.clients, &BridgeEvent::SettingsError { context: "brightness_set".into(), message }),
            }
        }
        ClientCommand::TimezoneGet {} => send_timezone(state),
        ClientCommand::TimezoneSet { value } => {
            match timezone::set_timezone(&value, &state.timezones) {
                Ok(()) => {
                    let mut settings = lock(&state.settings);
                    settings.timezone = Some(value);
                    save_settings(&settings);
                    drop(settings);
                    send_timezone(state);
                }
                Err(AppError { message, .. }) => broadcast(&state.clients, &BridgeEvent::SettingsError { context: "timezone_set".into(), message }),
            }
        }
        ClientCommand::TimezoneListGet {} => {
            broadcast(&state.clients, &BridgeEvent::TimezoneList { zones: (*state.timezones).clone() });
        }
        ClientCommand::WifiGet {} => state.wifi.announce(),
        // Scan and connect block for seconds (event waits, DHCP), so they
        // run off the WS-read thread like bt_pair/bt_connect above.
        ClientCommand::WifiScan {} => {
            let w = state.wifi.clone();
            thread::spawn(move || w.scan());
        }
        ClientCommand::WifiConnect { ssid, password } => {
            let w = state.wifi.clone();
            thread::spawn(move || w.connect(ssid, password.expose().to_string()));
        }
        ClientCommand::WifiConnectSaved { ssid } => {
            let w = state.wifi.clone();
            thread::spawn(move || w.connect_saved(ssid));
        }
        ClientCommand::WifiDisconnect {} => state.wifi.disconnect(),
        ClientCommand::WifiSavedListGet {} => state.wifi.list_saved(),
        ClientCommand::WifiForget { ssid } => state.wifi.forget(&ssid),
    }
}

fn send_brightness(state: &AppState) {
    match (backlight::read_current(), backlight::read_max()) {
        (Ok(value), Ok(max)) => broadcast(&state.clients, &BridgeEvent::Brightness { value, max }),
        (res1, res2) => {
            let message = res1.and(res2).unwrap_err().message;
            broadcast(&state.clients, &BridgeEvent::SettingsError { context: "brightness_get".into(), message });
        }
    }
}

fn send_timezone(state: &AppState) {
    match timezone::get_timezone() {
        Ok(value) => broadcast(&state.clients, &BridgeEvent::Timezone { value }),
        Err(AppError { message, .. }) => broadcast(&state.clients, &BridgeEvent::SettingsError { context: "timezone_get".into(), message }),
    }
}

/// Opens an evdev device by path, retrying for a few seconds - the device nodes exist
/// at boot, but this keeps the daemon resilient if it's started slightly early or the
/// node is briefly busy.
fn open_device(path: &str, cfg: &InputConfig) -> Device {
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

fn run_buttons(cfg: InputConfig, clients: Clients, mut uinput_dev: VirtualDevice) {
    let path = cfg.buttons_device.as_str();
    let long_press = Duration::from_millis(cfg.longpress_ms);
    let mut device = open_device(path, &cfg);
    match device.grab() {
        Ok(()) => info!("buttons: grabbed {path} exclusively (EVIOCGRAB)"),
        Err(e) => error!(
            "buttons: FAILED to grab {path}: {e} -- buttons 1-4/ESC will leak \
             raw keydowns to the browser until this succeeds"
        ),
    }
    info!("buttons: listening on {path} ({})", device.name().unwrap_or("?"));
    // KEY_M long-press state, shared with a per-press timer thread spawned
    // below: the action fires the moment the hold crosses KEY_M_LONG_PRESS
    // while still held, so the decision cannot be a local read at release
    // time. `LongPress` hands out a generation per press; a stale timer
    // (released or superseded) is a silent no-op. See longpress.rs.
    let key_m_press: Arc<Mutex<LongPress>> = Arc::new(Mutex::new(LongPress::new(long_press)));
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
                                error!("uinput emit failed for code={code}: {e}");
                            }
                        }

                        // KEY_M long-press hard-recovery: restart terminal-agent
                        // directly from here, no WS round-trip, no Chromium/
                        // terminal-agent dependency on the critical path. Fires
                        // the MOMENT the hold crosses KEY_M_LONG_PRESS, while the
                        // key is still physically held - not on release - matching
                        // the frontend ESC long-press UX. `fetch_events()` must
                        // keep running to notice a release/new-press, so the delay
                        // can't block this loop: on press, spawn a short-lived
                        // timer thread that sleeps KEY_M_LONG_PRESS then re-checks
                        // `key_m_press_state` still holds *this exact* press's
                        // Instant (see the field's doc comment above for why that
                        // generation check, not a plain `is_some()`, is what makes
                        // this race-safe) before firing.
                        if code == KEY_M {
                            if value == 1 {
                                let (generation, _deadline) = lock(&key_m_press).on_press(Instant::now());
                                let press_for_timer = key_m_press.clone();
                                let unit = cfg.restart_unit.clone();
                                thread::spawn(move || {
                                    thread::sleep(long_press);
                                    // should_fire claims the press, so a subsequent
                                    // release does not also act on it.
                                    if lock(&press_for_timer).should_fire(Instant::now(), generation) {
                                        info!("KEY_M long-press threshold reached while held: restarting {unit}");
                                        match Command::new("systemctl").args(["restart", unit.as_str()]).spawn() {
                                            Ok(_) => {}
                                            Err(e) => error!("failed to spawn systemctl restart {unit}: {e}"),
                                        }
                                    }
                                });
                            } else if value == 0 {
                                // Released before the timer fired (or after it
                                // already claimed the press): cancel.
                                lock(&key_m_press).on_release();
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
                            warn!("unexpected key code on event0: {code}");
                        }
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

fn run_knob(cfg: InputConfig, clients: Clients) {
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
                        broadcast(&clients, &BridgeEvent::Knob { delta: ev.value() });
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
    let (write_stream, shutdown_stream) = match (stream.try_clone(), stream.try_clone()) {
        (Ok(w), Ok(c)) => (w, c),
        (Err(e), _) | (_, Err(e)) => {
            error!("failed to clone client socket: {e}");
            return;
        }
    };
    let mut ws_read = match accept(stream) {
        Ok(s) => s,
        Err(e) => {
            warn!("websocket handshake failed: {e}");
            return;
        }
    };
    let mut ws_write = WebSocket::from_raw_socket(write_stream, Role::Server, None);

    let (tx, rx) = sync_channel::<String>(state.server.client_queue_capacity);
    lock(&state.clients).push(ClientSlot { tx, stream: Some(shutdown_stream) });
    info!("ui client connected");

    let ping_interval = Duration::from_secs(state.server.ping_interval_secs);
    thread::spawn(move || loop {
        match rx.recv_timeout(ping_interval) {
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
                    Ok(cmd) => {
                        // `Secret` renders as <redacted>, so this is safe to log.
                        debug!("client command: {cmd:?}");
                        handle_client_command(cmd, &state)
                    }
                    Err(e) => {
                        // Fixed text only: serde's message can echo field values
                        // (e.g. a mistyped wifi password), so it is neither logged
                        // nor sent. The error carries only the line/column.
                        warn!("ignoring unrecognized client message (line {}, column {})", e.line(), e.column());
                        broadcast(
                            &state.clients,
                            &BridgeEvent::SettingsError {
                                context: "client_message".into(),
                                message: "unrecognized or malformed message".into(),
                            },
                        );
                    }
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => continue, // binary/ping/pong - not part of this protocol
            Err(_) => break,
        }
    }
    info!("ui client disconnected");
    // The writer thread's channel sender still lives in `state.clients`;
    // `broadcast` prunes it on the next send once the writer has exited.
}

/// Runs the daemon with an already loaded configuration. The caller (`main`)
/// has installed the logger; this never returns.
pub fn run(loaded: utils::config::Loaded) {
    use utils::config::Source;
    match &loaded.source {
        Source::File(path) => info!("configuration loaded from {}", path.display()),
        Source::Defaults(path) => warn!("config file {} not found, using built-in defaults", path.display()),
    }
    let cfg: Config = loaded.config;
    let clients: Clients = Arc::new(Mutex::new(Vec::new()));

    let uinput_dev = build_uinput_passthrough();
    info!("uinput passthrough device ready (KEY_M, KEY_ENTER only)");

    let buttons_clients = clients.clone();
    let input_cfg = cfg.input.clone();
    thread::spawn(move || run_buttons(input_cfg, buttons_clients, uinput_dev));

    let knob_clients = clients.clone();
    let input_cfg = cfg.input.clone();
    thread::spawn(move || run_knob(input_cfg, knob_clients));

    // --- Settings-menu backend (docs/research/settings-menu-feasibility.md) ---

    let settings = load_settings();
    info!("settings: loaded {settings:?} from {}", settings::SETTINGS_PATH);

    // Brightness persistence (§8.1): systemd-backlight@backlight:backlight.service
    // IS present and active on this image, but its saved-state directory
    // (/var/lib/systemd/backlight/) was found empty on a live check - its
    // "save" half is unconfirmed to actually fire on this BSP's
    // reboot/poweroff path, so this agent re-applies its own persisted
    // value at startup too (belt-and-suspenders, not a replacement).
    if let Some(value) = settings.brightness {
        match backlight::set(value) {
            Ok(()) => info!("brightness: re-applied saved value {value} at startup"),
            Err(e) => error!("brightness: failed to re-apply saved value {value} at startup: {e}"),
        }
    }

    // Timezone persistence (§7.2): `timedatectl set-timezone`'s own
    // /etc/localtime symlink write does not survive a reboot on this
    // read-only-rootfs image, so re-apply unconditionally at every startup.
    let timezones = Arc::new(timezone::list_timezones());
    info!("timezone: {} valid IANA zones cached at startup", timezones.len());
    if let Some(ref tz) = settings.timezone {
        match timezone::set_timezone(tz, &timezones) {
            Ok(()) => info!("timezone: re-applied saved value {tz} at startup"),
            Err(e) => error!("timezone: failed to re-apply saved value {tz} at startup: {e}"),
        }
    }

    let (bt_events_tx, bt_events_rx) = channel::<bluetooth::AgentEvent>();
    // Connect/register BEFORE spawning the relay thread, not after: the
    // relay thread needs the resulting handle (to refresh bt_paired_list
    // after a successful pairing), so it can't be spawned with only the
    // yet-to-exist `bt` value.
    let bt = match bluetooth::connect_and_register_agent(bt_events_tx.clone()) {
        Ok(handle) => {
            info!("bt: connected to system bus, agent registered");
            Some(Arc::new(handle))
        }
        Err(e) => {
            error!(
                "bt: D-Bus connection/agent registration failed: {e} -- Bluetooth \
                 commands will return bt_error until this is resolved; buttons/knob/\
                 brightness/timezone are unaffected"
            );
            None
        }
    };
    {
        let clients = clients.clone();
        let bt_for_relay = bt.clone();
        thread::spawn(move || relay_bt_events(clients, bt_for_relay, bt_events_rx));
    }

    // Wi-Fi: events from the service object are mapped onto the wire here;
    // the watcher thread owns adapter hotplug detection and status polling.
    let (wifi_tx, wifi_rx) = channel::<wifi::WifiEvent>();
    let wifi = Arc::new(wifi::Wifi::with_config(wifi_tx, cfg.wifi.clone()));
    wifi::ensure_config(&cfg.wifi);
    {
        let clients = clients.clone();
        thread::spawn(move || {
            for event in wifi_rx {
                broadcast(&clients, &BridgeEvent::from(event));
            }
        });
    }
    {
        let wifi = wifi.clone();
        thread::spawn(move || wifi.run_watcher());
    }

    let state = Arc::new(AppState {
        clients: clients.clone(),
        bt,
        bt_events: bt_events_tx,
        settings: Arc::new(Mutex::new(settings)),
        timezones,
        wifi,
        server: cfg.server.clone(),
    });

    let addr = bind_addr(std::env::var(BIND_ENV).ok().as_deref(), cfg.bind_addr());
    if !is_loopback(&addr) {
        warn!(
            "WARNING: the configured bind ({BIND_ENV} or server.bind) is {addr}, which is NOT loopback. The \
             WebSocket API is unauthenticated (Wi-Fi credentials, Bluetooth, settings) and will be reachable \
             from the network. Development only."
        );
    }
    let listener = TcpListener::bind(addr).unwrap_or_else(|e| panic!("bind {addr} failed: {e}"));
    info!("WebSocket server listening on ws://{addr}");

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let state = state.clone();
                thread::spawn(move || handle_client(stream, state));
            }
            Err(e) => warn!("accept error: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_bind() -> SocketAddr {
        Config::default().bind_addr()
    }

    fn test_clients() -> Clients {
        Arc::new(Mutex::new(Vec::new()))
    }

    fn add_client(clients: &Clients, cap: usize) -> std::sync::mpsc::Receiver<String> {
        let (tx, rx) = sync_channel::<String>(cap);
        lock(clients).push(ClientSlot { tx, stream: None });
        rx
    }

    #[test]
    fn stalled_client_is_evicted_when_its_queue_fills_and_healthy_client_is_unaffected() {
        let clients = test_clients();
        let stalled = add_client(&clients, 4);
        let healthy = add_client(&clients, 64);
        let start = Instant::now();
        for i in 0..10 {
            broadcast(&clients, &BridgeEvent::Knob { delta: i });
            // The healthy client drains as it goes; the stalled one never reads.
            assert!(healthy.try_recv().is_ok());
        }
        assert!(start.elapsed() < Duration::from_secs(1), "broadcast must never block");
        assert_eq!(lock(&clients).len(), 1, "stalled client evicted, healthy kept");
        // Evicted client got exactly its queue capacity, then nothing more.
        assert_eq!(stalled.try_iter().count(), 4);
        assert!(matches!(stalled.try_recv(), Err(std::sync::mpsc::TryRecvError::Disconnected)));
    }

    #[test]
    fn dead_client_is_pruned() {
        let clients = test_clients();
        drop(add_client(&clients, 4));
        broadcast(&clients, &BridgeEvent::Knob { delta: 1 });
        assert!(lock(&clients).is_empty());
    }

    #[test]
    fn eviction_closes_the_client_socket() {
        use std::io::Read;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut peer = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server_side, _) = listener.accept().unwrap();
        let clients = test_clients();
        let (tx, _rx) = sync_channel::<String>(1);
        lock(&clients).push(ClientSlot { tx, stream: Some(server_side) });
        broadcast(&clients, &BridgeEvent::Knob { delta: 1 });
        broadcast(&clients, &BridgeEvent::Knob { delta: 2 }); // queue (cap 1) full: evict
        assert!(lock(&clients).is_empty());
        let mut buf = [0u8; 1];
        assert_eq!(peer.read(&mut buf).unwrap(), 0, "peer sees EOF after shutdown");
    }

    #[test]
    fn broadcast_survives_a_poisoned_client_list() {
        let clients = test_clients();
        let c2 = clients.clone();
        let _ = thread::spawn(move || {
            let _g = c2.lock().unwrap();
            panic!("poison");
        })
        .join();
        let rx = add_client(&clients, 4);
        broadcast(&clients, &BridgeEvent::Knob { delta: 1 });
        assert_eq!(rx.try_recv().unwrap(), r#"{"type":"knob","delta":1}"#);
    }

    #[test]
    fn high_frequency_events_are_not_logged_and_passkeys_are_redacted() {
        assert!(matches!(log_policy(&BridgeEvent::Knob { delta: 1 }), LogPolicy::Silent));
        assert!(matches!(log_policy(&BridgeEvent::Button { code: 1, value: 1 }), LogPolicy::Silent));
        let status = BridgeEvent::from(wifi::WifiEvent::Status(wifi::StatusInfo {
            state: wifi::ConnState::Connected,
            ssid: None,
            ip: None,
            interface: "wlan0".into(),
            signal_pct: None,
            ethernet_up: false,
        }));
        assert!(matches!(log_policy(&status), LogPolicy::Silent));
        let pk = BridgeEvent::BtPairingConfirmRequest { address: "AA".into(), passkey: 123456 };
        assert!(matches!(log_policy(&pk), LogPolicy::TypeOnly));
        assert!(matches!(log_policy(&BridgeEvent::BtPower { powered: true }), LogPolicy::Full));
    }

    #[test]
    fn default_bind_is_loopback() {
        let addr = bind_addr(None, default_bind());
        assert!(is_loopback(&addr), "{addr}");
        assert_eq!(addr.port(), 8765);
        assert!(is_loopback(&DEFAULT_BIND.parse().unwrap()));
    }

    #[test]
    fn bind_env_override_is_honoured_and_flagged_when_wide() {
        let wide = bind_addr(Some("0.0.0.0:9000"), default_bind());
        assert_eq!(wide.port(), 9000);
        assert!(!is_loopback(&wide));
        assert!(is_loopback(&bind_addr(Some("[::1]:8765"), default_bind())));
    }

    #[test]
    fn invalid_bind_env_falls_back_to_loopback_default() {
        assert_eq!(bind_addr(Some("not an address"), default_bind()), default_bind());
        assert!(is_loopback(&bind_addr(Some(""), default_bind())));
    }

    #[test]
    fn listener_binds_loopback_by_default() {
        let mut addr = bind_addr(None, default_bind());
        addr.set_port(0);
        let l = TcpListener::bind(addr).unwrap();
        assert!(l.local_addr().unwrap().ip().is_loopback());
    }

}
