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

use log::{debug, error, info, warn};
use utils::config::{Config, InputConfig, ServerConfig};
use evdev::uinput::{VirtualDevice, VirtualDeviceBuilder};
use evdev::{AttributeSet, Device, EventType, InputEvent, Key};
use adapters::json_settings::JsonSettings;
use adapters::sysfs_backlight::SysfsBacklight;
use adapters::systemctl::SystemctlRestarter;
use adapters::timedatectl::Timedatectl;
use app::hub::EventHub;
use app::router::Router;
use adapters::bluez::BluezBluetooth;
use adapters::unavailable_bluetooth::UnavailableBluetooth;
use domain::bluetooth::AgentEvent;
use services::bluetooth::BluetoothService;
use ports::{BluetoothPort, EventSink, Restarter, SettingsStore};
use services::display::DisplayService;
use services::settings::SettingsHolder;
use services::timezone::TimezoneService;
use protocol::{BridgeEvent, ClientCommand};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::sync::mpsc::{channel, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tungstenite::protocol::{Role, WebSocket};
use tungstenite::{accept, Message};

pub mod domain;
pub mod sync_util;
pub mod wifi;
pub mod utils;
pub mod protocol;
pub mod app;
pub mod ports;
pub mod services;
pub mod adapters;

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

/// What a client connection needs: the hub to register with and the router
/// its commands go to.
struct AppState {
    clients: Arc<EventHub>,
    router: Arc<Router>,
    server: ServerConfig,
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

fn run_buttons(cfg: InputConfig, clients: Arc<EventHub>, restarter: Arc<dyn Restarter>, mut uinput_dev: VirtualDevice) {
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
                        clients.emit(BridgeEvent::Button { code, value });

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
                                let restarter = restarter.clone();
                                thread::spawn(move || {
                                    thread::sleep(long_press);
                                    // should_fire claims the press, so a subsequent
                                    // release does not also act on it.
                                    if lock(&press_for_timer).should_fire(Instant::now(), generation) {
                                        info!("KEY_M long-press threshold reached while held: restarting {unit}");
                                        if let Err(e) = restarter.restart(&unit) {
                                            error!("{e}");
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

fn run_knob(cfg: InputConfig, clients: Arc<EventHub>) {
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
                        clients.emit(BridgeEvent::Knob { delta: ev.value() });
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
    state.clients.register(tx, Some(shutdown_stream));
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
                        state.router.handle(cmd)
                    }
                    Err(e) => {
                        // Fixed text only: serde's message can echo field values
                        // (e.g. a mistyped wifi password), so it is neither logged
                        // nor sent. The error carries only the line/column.
                        warn!("ignoring unrecognized client message (line {}, column {})", e.line(), e.column());
                        state.clients.emit(BridgeEvent::SettingsError {
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
    let clients = Arc::new(EventHub::new());

    let uinput_dev = build_uinput_passthrough();
    info!("uinput passthrough device ready (KEY_M, KEY_ENTER only)");

    let buttons_clients = clients.clone();
    let input_cfg = cfg.input.clone();
    let restarter: Arc<dyn Restarter> = Arc::new(SystemctlRestarter::new());
    thread::spawn(move || run_buttons(input_cfg, buttons_clients, restarter, uinput_dev));

    let knob_clients = clients.clone();
    let input_cfg = cfg.input.clone();
    thread::spawn(move || run_knob(input_cfg, knob_clients));

    // --- Settings-menu backend (docs/research/settings-menu-feasibility.md) ---

    let store: Arc<dyn SettingsStore> = Arc::new(JsonSettings::new(&cfg.settings));
    let settings = Arc::new(SettingsHolder::load(store));
    info!("settings: loaded {:?} from {}", settings.snapshot(), cfg.settings.path);
    let sink: Arc<dyn EventSink> = clients.clone();

    let display = DisplayService::new(Arc::new(SysfsBacklight::new(&cfg.display)), settings.clone(), sink.clone());
    display.apply_saved();
    let timezone = TimezoneService::new(Arc::new(Timedatectl::new(&cfg.timezone)), settings.clone(), sink.clone());
    timezone.apply_saved();

    let (bt_events_tx, bt_events_rx) = channel::<AgentEvent>();
    let bt_port: Arc<dyn BluetoothPort> = match BluezBluetooth::connect(bt_events_tx.clone(), &cfg.bluetooth) {
        Ok(port) => {
            info!("bt: connected to system bus, agent registered");
            Arc::new(port)
        }
        Err(e) => {
            error!(
                "bt: D-Bus connection/agent registration failed: {e} -- Bluetooth \
                 commands will return bt_error until this is resolved; buttons/knob/\
                 brightness/timezone are unaffected"
            );
            Arc::new(UnavailableBluetooth)
        }
    };
    let bt = Arc::new(BluetoothService::new(bt_port, sink.clone(), bt_events_tx));
    {
        let bt = bt.clone();
        thread::spawn(move || bt.relay(bt_events_rx));
    }

    // Wi-Fi: events from the service object are mapped onto the wire here;
    // the watcher thread owns adapter hotplug detection and status polling.
    let (wifi_tx, wifi_rx) = channel::<wifi::WifiEvent>();
    let wifi = Arc::new(wifi::Wifi::with_config(wifi_tx, cfg.wifi.clone()));
    wifi::ensure_config(&cfg.wifi);
    {
        let sink = sink.clone();
        thread::spawn(move || {
            for event in wifi_rx {
                sink.emit(BridgeEvent::from(event));
            }
        });
    }
    {
        let wifi = wifi.clone();
        thread::spawn(move || wifi.run_watcher());
    }

    let router = Arc::new(Router::new(bt, wifi, display, timezone));
    let state = Arc::new(AppState { clients: clients.clone(), router, server: cfg.server.clone() });

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
