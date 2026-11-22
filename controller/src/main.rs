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

use evdev::uinput::{VirtualDevice, VirtualDeviceBuilder};
use evdev::{AttributeSet, Device, EventType, InputEvent, Key};
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
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
mod wifi;

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

/// Default WebSocket bind. The only legitimate client is the Chromium kiosk
/// on the device itself (frontend/app.js connects to ws://localhost:8765),
/// and this socket can change Wi-Fi, Bluetooth and system settings with no
/// authentication, so it must not be reachable from the network. From a dev
/// host use `ssh -L 8765:127.0.0.1:8765 <device>` instead of a wide bind.
const DEFAULT_BIND: &str = "127.0.0.1:8765";

/// Env var a developer can set to deliberately override `DEFAULT_BIND`
/// (e.g. `CONTROLLER_BIND=0.0.0.0:8765`). A non-loopback value is honoured
/// but logged loudly in `main`. An unparsable value falls back to the
/// loopback default rather than to anything wider.
const BIND_ENV: &str = "CONTROLLER_BIND";

/// Resolves the listen address from the raw env value (`None` = unset).
fn bind_addr(env_value: Option<&str>) -> SocketAddr {
    let default: SocketAddr = DEFAULT_BIND.parse().expect("DEFAULT_BIND is a valid socket address");
    match env_value {
        None => default,
        Some(raw) => raw.trim().parse().unwrap_or_else(|e| {
            eprintln!("[bridge] {BIND_ENV}={raw:?} is not a valid host:port ({e}); using {DEFAULT_BIND}");
            default
        }),
    }
}

fn is_loopback(addr: &SocketAddr) -> bool {
    match addr.ip() {
        IpAddr::V4(ip) => ip.is_loopback(),
        IpAddr::V6(ip) => ip.is_loopback(),
    }
}

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
    /// The full current set of paired devices - sent in reply to
    /// `bt_paired_list_get`, and proactively after anything that could
    /// change the paired set (a successful new pairing, or a successful
    /// `bt_connect`/`bt_disconnect`/`bt_forget`) so the frontend's list
    /// stays in sync without a manual re-request.
    #[serde(rename = "bt_paired_list")]
    BtPairedList { devices: Vec<PairedDeviceWire> },

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

    // --- Wi-Fi (docs/wifi.md). No event ever carries a password. ---
    /// A wireless interface appeared or disappeared (adapter hotplug).
    /// The menu shows the Wi-Fi item only while `available` is true.
    #[serde(rename = "wifi_available")]
    WifiAvailable { available: bool, interface: Option<String> },
    /// Current connection state. `state` is one of `connected`,
    /// `obtaining_ip`, `connecting`, `disconnected`. `ssid`/`ip`/
    /// `signal_pct` are `None` when not applicable; `ethernet_up` is true
    /// when a wired link is also up.
    #[serde(rename = "wifi_status")]
    WifiStatus {
        state: String,
        ssid: Option<String>,
        ip: Option<String>,
        interface: String,
        signal_pct: Option<u8>,
        ethernet_up: bool,
    },
    #[serde(rename = "wifi_scan_started")]
    WifiScanStarted {},
    /// Deduplicated by SSID (strongest kept), strongest first. `security`
    /// is `open`, `wpa2`, `wpa3`, `wpa2/wpa3`, `wep` or `eap`; `supported`
    /// is false for `wep`/`eap`.
    #[serde(rename = "wifi_scan_results")]
    WifiScanResults { networks: Vec<WifiNetworkWire> },
    #[serde(rename = "wifi_connecting")]
    WifiConnecting { ssid: String },
    /// Final outcome of `wifi_connect`/`wifi_connect_saved`. On failure
    /// `error` is a stable code (`wrong_password`, `network_not_found`,
    /// `assoc_rejected`, `auth_rejected`, `no_response`, `dhcp_timeout`,
    /// `unavailable`, `invalid_input`) and `message` is display text.
    #[serde(rename = "wifi_connect_result")]
    WifiConnectResult { ssid: String, success: bool, error: Option<String>, message: Option<String> },
    #[serde(rename = "wifi_saved_list")]
    WifiSavedList { networks: Vec<WifiSavedWire> },
    #[serde(rename = "wifi_error")]
    WifiError { context: String, message: String },
}

#[derive(Serialize, Clone, Debug)]
struct WifiNetworkWire {
    ssid: String,
    signal_pct: u8,
    signal_dbm: i32,
    security: String,
    supported: bool,
}

#[derive(Serialize, Clone, Debug)]
struct WifiSavedWire {
    ssid: String,
    current: bool,
}

impl From<wifi::WifiEvent> for BridgeEvent {
    fn from(e: wifi::WifiEvent) -> Self {
        use wifi::WifiEvent as W;
        match e {
            W::Available { available, interface } => BridgeEvent::WifiAvailable { available, interface },
            W::Status(s) => BridgeEvent::WifiStatus {
                state: s.state.wire().into(),
                ssid: s.ssid,
                ip: s.ip,
                interface: s.interface,
                signal_pct: s.signal_pct,
                ethernet_up: s.ethernet_up,
            },
            W::ScanStarted => BridgeEvent::WifiScanStarted {},
            W::ScanResults(list) => BridgeEvent::WifiScanResults {
                networks: list
                    .into_iter()
                    .map(|n| WifiNetworkWire {
                        ssid: n.ssid,
                        signal_pct: n.signal_pct,
                        signal_dbm: n.signal_dbm,
                        security: n.security.wire().into(),
                        supported: n.security.supported(),
                    })
                    .collect(),
            },
            W::Connecting { ssid } => BridgeEvent::WifiConnecting { ssid },
            W::ConnectResult { ssid, error } => BridgeEvent::WifiConnectResult {
                ssid,
                success: error.is_none(),
                message: error.as_ref().map(|e| e.message()),
                error: error.map(|e| e.code().to_string()),
            },
            W::SavedList(list) => BridgeEvent::WifiSavedList {
                networks: list.into_iter().map(|n| WifiSavedWire { ssid: n.ssid, current: n.current }).collect(),
            },
            W::Error { context, message } => BridgeEvent::WifiError { context, message },
        }
    }
}

/// A string that must never reach a log line: its `Debug` output is fixed.
/// `ClientCommand` derives `Debug`, and a password-carrying variant must
/// not leak through a stray `{:?}`.
#[derive(Deserialize, Clone, Default)]
#[serde(transparent)]
struct Secret(String);

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<redacted>")
    }
}

/// Wire shape for one entry of `BridgeEvent::BtPairedList`. A thin
/// serializable mirror of `bluetooth::PairedDevice` - kept as a separate
/// struct here (not a `derive(Serialize)` on `bluetooth::PairedDevice`
/// itself) for the same reason `BtDeviceFound` doesn't reuse
/// `bluetooth::DiscoveredDevice` directly: `bluetooth.rs`'s own doc
/// comment states it deliberately has no WS/serde dependency, main.rs is
/// where the D-Bus-facing data model meets the wire format.
#[derive(Serialize, Clone, Debug)]
struct PairedDeviceWire {
    address: String,
    name: Option<String>,
    connected: bool,
}

impl From<bluetooth::PairedDevice> for PairedDeviceWire {
    fn from(d: bluetooth::PairedDevice) -> Self {
        PairedDeviceWire { address: d.address, name: d.name, connected: d.connected }
    }
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
    #[serde(rename = "bt_paired_list_get")]
    BtPairedListGet {},
    #[serde(rename = "bt_connect")]
    BtConnect { address: String },
    #[serde(rename = "bt_disconnect")]
    BtDisconnect { address: String },
    #[serde(rename = "bt_forget")]
    BtForget { address: String },
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
    #[serde(rename = "wifi_get")]
    WifiGet {},
    #[serde(rename = "wifi_scan")]
    WifiScan {},
    /// `password` is empty for open networks. Security is taken from the
    /// agent's own last scan, not from the client.
    #[serde(rename = "wifi_connect")]
    WifiConnect { ssid: String, #[serde(default)] password: Secret },
    #[serde(rename = "wifi_connect_saved")]
    WifiConnectSaved { ssid: String },
    #[serde(rename = "wifi_disconnect")]
    WifiDisconnect {},
    #[serde(rename = "wifi_saved_list_get")]
    WifiSavedListGet {},
    #[serde(rename = "wifi_forget")]
    WifiForget { ssid: String },
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
    wifi: Arc<wifi::Wifi>,
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
        Err(message) => broadcast(clients, &BridgeEvent::BtError { context: "bt_paired_list_get".into(), message }),
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
                    Err(message) => broadcast(&clients, &BridgeEvent::BtError { context: "bt_connect".into(), message }),
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
                    Err(message) => broadcast(&clients, &BridgeEvent::BtError { context: "bt_disconnect".into(), message }),
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
                Err(message) => broadcast(&state.clients, &BridgeEvent::BtError { context: "bt_forget".into(), message }),
            },
            None => broadcast(&state.clients, &BridgeEvent::BtError { context: "bt_forget".into(), message: "Bluetooth unavailable".into() }),
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
        ClientCommand::WifiGet {} => state.wifi.announce(),
        // Scan and connect block for seconds (event waits, DHCP), so they
        // run off the WS-read thread like bt_pair/bt_connect above.
        ClientCommand::WifiScan {} => {
            let w = state.wifi.clone();
            thread::spawn(move || w.scan());
        }
        ClientCommand::WifiConnect { ssid, password } => {
            let w = state.wifi.clone();
            thread::spawn(move || w.connect(ssid, password.0));
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
    // Tracks the Instant of the currently-active KEY_M press, shared with a
    // per-press timer thread spawned below. The long-press action now fires
    // the moment the hold crosses KEY_M_LONG_PRESS *while still held*, not
    // on release - so this can't be a simple local `Option<Instant>` read
    // only at release time anymore; it needs to be checked from a second
    // thread that doesn't block `fetch_events()`. The Instant stored here
    // also acts as a generation marker: a spawned timer only fires if, after
    // its sleep, this field still holds the *exact same* Instant it captured
    // at spawn time - if a release (or a new press) has since changed/cleared
    // it, the stale timer is a silent no-op, so a rapid
    // press-release-press can't let the first press's delayed timer fire
    // after the second press has already started.
    let key_m_press_state: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
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
                                let this_press = Instant::now();
                                *key_m_press_state.lock().unwrap() = Some(this_press);
                                let state_for_timer = key_m_press_state.clone();
                                thread::spawn(move || {
                                    thread::sleep(KEY_M_LONG_PRESS);
                                    let mut guard = state_for_timer.lock().unwrap();
                                    if *guard == Some(this_press) {
                                        // Still the same, still-held press: claim it
                                        // (clear the shared state) so a subsequent
                                        // release doesn't also try to act on it, then
                                        // fire the hard-recovery action.
                                        *guard = None;
                                        drop(guard);
                                        println!(
                                            "[bridge] KEY_M long-press threshold reached while held: restarting {TERMINAL_AGENT_UNIT}"
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
                                    // Else: a release or a newer press already
                                    // superseded this one - stale timer, no-op.
                                });
                            } else if value == 0 {
                                // Short tap (the long-press timer above hasn't fired
                                // yet, or this press was already superseded): just
                                // clear the shared state. No elapsed-check here
                                // anymore - the timer thread now owns that decision.
                                *key_m_press_state.lock().unwrap() = None;
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
    // Connect/register BEFORE spawning the relay thread, not after: the
    // relay thread needs the resulting handle (to refresh bt_paired_list
    // after a successful pairing), so it can't be spawned with only the
    // yet-to-exist `bt` value.
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
    {
        let clients = clients.clone();
        let bt_for_relay = bt.clone();
        thread::spawn(move || relay_bt_events(clients, bt_for_relay, bt_events_rx));
    }

    // Wi-Fi: events from the service object are mapped onto the wire here;
    // the watcher thread owns adapter hotplug detection and status polling.
    let (wifi_tx, wifi_rx) = channel::<wifi::WifiEvent>();
    let wifi = Arc::new(wifi::Wifi::new(wifi_tx));
    wifi::ensure_config();
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
    });

    let addr = bind_addr(std::env::var(BIND_ENV).ok().as_deref());
    if !is_loopback(&addr) {
        eprintln!(
            "[bridge] WARNING: {BIND_ENV} binds {addr}, which is NOT loopback. The WebSocket API is \
             unauthenticated (Wi-Fi credentials, Bluetooth, settings) and will be reachable from the \
             network. Development only."
        );
    }
    let listener = TcpListener::bind(addr).unwrap_or_else(|e| panic!("bind {addr} failed: {e}"));
    println!("[bridge] WebSocket server listening on ws://{addr}");

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_bind_is_loopback() {
        let addr = bind_addr(None);
        assert!(is_loopback(&addr), "{addr}");
        assert_eq!(addr.port(), 8765);
        assert!(is_loopback(&DEFAULT_BIND.parse().unwrap()));
    }

    #[test]
    fn bind_env_override_is_honoured_and_flagged_when_wide() {
        let wide = bind_addr(Some("0.0.0.0:9000"));
        assert_eq!(wide.port(), 9000);
        assert!(!is_loopback(&wide));
        assert!(is_loopback(&bind_addr(Some("[::1]:8765"))));
    }

    #[test]
    fn invalid_bind_env_falls_back_to_loopback_default() {
        assert_eq!(bind_addr(Some("not an address")), bind_addr(None));
        assert!(is_loopback(&bind_addr(Some(""))));
    }

    #[test]
    fn listener_binds_loopback_by_default() {
        let mut addr = bind_addr(None);
        addr.set_port(0);
        let l = TcpListener::bind(addr).unwrap();
        assert!(l.local_addr().unwrap().ip().is_loopback());
    }

    #[test]
    fn wifi_commands_parse() {
        let c: ClientCommand = serde_json::from_str(r#"{"type":"wifi_scan"}"#).unwrap();
        assert!(matches!(c, ClientCommand::WifiScan {}));
        let c: ClientCommand = serde_json::from_str(r#"{"type":"wifi_forget","ssid":"Home"}"#).unwrap();
        assert!(matches!(c, ClientCommand::WifiForget { ssid } if ssid == "Home"));
        let c: ClientCommand = serde_json::from_str(r#"{"type":"wifi_connect","ssid":"Home","password":"hunter2hunter2"}"#).unwrap();
        match c {
            ClientCommand::WifiConnect { ssid, password } => {
                assert_eq!(ssid, "Home");
                assert_eq!(password.0, "hunter2hunter2");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn wifi_connect_password_defaults_to_empty_for_open_networks() {
        let c: ClientCommand = serde_json::from_str(r#"{"type":"wifi_connect","ssid":"Cafe"}"#).unwrap();
        assert!(matches!(c, ClientCommand::WifiConnect { password, .. } if password.0.is_empty()));
    }

    #[test]
    fn wifi_connect_missing_ssid_and_unknown_types_are_rejected() {
        assert!(serde_json::from_str::<ClientCommand>(r#"{"type":"wifi_connect","password":"x"}"#).is_err());
        assert!(serde_json::from_str::<ClientCommand>(r#"{"type":"wifi_nope"}"#).is_err());
    }

    #[test]
    fn password_never_appears_in_debug_output() {
        let c: ClientCommand = serde_json::from_str(r#"{"type":"wifi_connect","ssid":"Home","password":"hunter2hunter2"}"#).unwrap();
        let dbg = format!("{c:?}");
        assert!(!dbg.contains("hunter2"), "{dbg}");
        assert!(dbg.contains("<redacted>"));
    }

    #[test]
    fn connect_result_event_wire_shape() {
        let ok = BridgeEvent::from(wifi::WifiEvent::ConnectResult { ssid: "Home".into(), error: None });
        let v: serde_json::Value = serde_json::to_value(&ok).unwrap();
        assert_eq!(v["type"], "wifi_connect_result");
        assert_eq!(v["success"], true);
        assert!(v["error"].is_null());

        let bad = BridgeEvent::from(wifi::WifiEvent::ConnectResult {
            ssid: "Home".into(),
            error: Some(wifi::FailKind::WrongPassword),
        });
        let v: serde_json::Value = serde_json::to_value(&bad).unwrap();
        assert_eq!(v["success"], false);
        assert_eq!(v["error"], "wrong_password");
        assert_eq!(v["message"], "Wrong password");
    }

    #[test]
    fn status_event_wire_shape() {
        let e = BridgeEvent::from(wifi::WifiEvent::Status(wifi::StatusInfo {
            state: wifi::ConnState::Connected,
            ssid: Some("Home".into()),
            ip: Some("192.168.1.9".into()),
            interface: "wlan0".into(),
            signal_pct: Some(80),
            ethernet_up: true,
        }));
        let v: serde_json::Value = serde_json::to_value(&e).unwrap();
        assert_eq!(v["type"], "wifi_status");
        assert_eq!(v["state"], "connected");
        assert_eq!(v["ethernet_up"], true);
        assert_eq!(v["signal_pct"], 80);
    }
}
