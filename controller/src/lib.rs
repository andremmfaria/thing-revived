//! controller-agent: hardware-input-to-WebSocket bridge. Reads the Car
//! Thing's buttons and knob, serves the settings backend (Bluetooth, Wi-Fi,
//! brightness, timezone) to the kiosk UI over a loopback WebSocket, and
//! turns a KEY_M long press into a restart of the terminal agent.
//!
//! Layering: `domain` (pure) <- `services`/`app` (use `ports` traits) <-
//! `adapters` (evdev, uinput, BlueZ, sysfs, systemctl, WebSocket). `run`
//! below is the only place that picks concrete adapters. See
//! docs/research/architecture-proposals/controller.md.

pub mod adapters;
pub mod app;
pub mod domain;
pub mod ports;
pub mod protocol;
pub mod services;
pub mod sync_util;
pub mod utils;

use adapters::accounts_shadow::{EtcFiles, EtcPaths, JsonStateFile, ShadowAccounts, DEFAULT_SHA_ROUNDS};
use adapters::bluez::BluezBluetooth;
use adapters::evdev_input::{run_buttons, run_knob};
use adapters::json_settings::JsonSettings;
use adapters::os_entropy::OsEntropy;
use adapters::sysfs_backlight::SysfsBacklight;
use adapters::systemctl::SystemctlRestarter;
use adapters::timedatectl::Timedatectl;
use adapters::uinput::UinputReplay;
use adapters::unavailable_bluetooth::UnavailableBluetooth;
use app::hub::EventHub;
use app::router::Router;
use domain::bluetooth::AgentEvent;
use log::{error, info, warn};
use ports::{BluetoothPort, EventSink, FirstBootGate, Restarter, SettingsStore};
use protocol::BridgeEvent;
use services::bluetooth::BluetoothService;
use services::display::DisplayService;
use services::input::{ButtonPolicy, SystemClock};
use services::settings::SettingsHolder;
use services::timezone::TimezoneService;
use services::users::{UsersPorts, UsersService};
use std::sync::mpsc::channel;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use utils::config::{Config, Loaded, Source};

/// Runs the daemon with an already loaded configuration. The caller (`main`)
/// has installed the logger; this never returns.
pub fn run(loaded: Loaded) {
    match &loaded.source {
        Source::File(path) => info!("configuration loaded from {}", path.display()),
        Source::Defaults(path) => warn!("config file {} not found, using built-in defaults", path.display()),
    }
    let cfg: Config = loaded.config;
    let hub = Arc::new(EventHub::new());
    let sink: Arc<dyn EventSink> = hub.clone();

    let users = start_users(&cfg, &hub);
    start_input(&cfg, &hub, users.gate());

    let store: Arc<dyn SettingsStore> = Arc::new(JsonSettings::new(&cfg.settings));
    let settings = Arc::new(SettingsHolder::load(store));
    info!("settings: loaded {:?} from {}", settings.snapshot(), cfg.settings.path);
    let display = DisplayService::new(Arc::new(SysfsBacklight::new(&cfg.display)), settings.clone(), sink.clone());
    let timezone = TimezoneService::new(Arc::new(Timedatectl::new(&cfg.timezone)), settings, sink.clone());
    app::startup::apply_persisted(&display, &timezone);

    let bt = start_bluetooth(&cfg, &sink);
    let wifi = start_wifi(&cfg, &sink);
    let router = Arc::new(Router::new(bt, wifi, display, timezone));
    router.attach_users(users);
    adapters::ws_server::serve(&cfg, hub, router);
}

/// First-boot password service. It decides lazily, at the first
/// `user_bootstrap_get`, so nothing runs until a frontend is there to show it.
fn start_users(cfg: &Config, hub: &Arc<EventHub>) -> Arc<UsersService> {
    let ports = UsersPorts {
        state: Arc::new(JsonStateFile::new(&cfg.users.state_dir)),
        files: Arc::new(EtcFiles::new(EtcPaths::default())),
        accounts: Arc::new(ShadowAccounts::new(DEFAULT_SHA_ROUNDS)),
        entropy: Arc::new(OsEntropy::new()),
        clock: Arc::new(SystemClock),
    };
    Arc::new(UsersService::new(ports, hub.clone(), hub.clone(), &cfg.users))
}

/// Grabs the buttons device (with selective uinput replay) and reads the knob.
/// The buttons policy asks `gate` on each M and knob-push press edge, so the
/// first-boot screen can own those two keys while it is up.
fn start_input(cfg: &Config, hub: &Arc<EventHub>, gate: Arc<dyn FirstBootGate>) {
    let uinput = UinputReplay::build();
    info!("uinput passthrough device ready (KEY_M, KEY_ENTER only)");
    let restarter: Arc<dyn Restarter> = Arc::new(SystemctlRestarter::new());
    let button_sink: Arc<dyn EventSink> = hub.clone();
    let policy = ButtonPolicy::new(
        button_sink,
        Box::new(uinput),
        restarter,
        Arc::new(SystemClock),
        cfg.input.restart_unit.clone(),
        Duration::from_millis(cfg.input.longpress_ms),
        gate,
    );
    let input_cfg = cfg.input.clone();
    thread::spawn(move || run_buttons(input_cfg, policy));
    let knob_sink: Arc<dyn EventSink> = hub.clone();
    let input_cfg = cfg.input.clone();
    thread::spawn(move || run_knob(input_cfg, knob_sink));
}

/// Connects to BlueZ, falling back to the unavailable adapter so the rest of
/// the daemon keeps working, and starts the agent-event relay thread.
fn start_bluetooth(cfg: &Config, sink: &Arc<dyn EventSink>) -> Arc<BluetoothService> {
    let (events_tx, events_rx) = channel::<AgentEvent>();
    let port: Arc<dyn BluetoothPort> = match BluezBluetooth::connect(events_tx.clone(), &cfg.bluetooth) {
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
    let bt = Arc::new(BluetoothService::new(port, sink.clone(), events_tx));
    let relay = bt.clone();
    thread::spawn(move || relay.relay(events_rx));
    bt
}

/// Wi-Fi: events from the service object are mapped onto the wire here; the
/// watcher thread owns adapter hotplug detection and status polling.
fn start_wifi(cfg: &Config, sink: &Arc<dyn EventSink>) -> Arc<services::wifi::Wifi> {
    let (tx, rx) = channel::<services::wifi::WifiEvent>();
    let wifi = Arc::new(services::wifi::Wifi::with_config(tx, cfg.wifi.clone()));
    adapters::wpa_ctrl::ensure_config(&cfg.wifi);
    let sink = sink.clone();
    thread::spawn(move || {
        for event in rx {
            sink.emit(BridgeEvent::from(event));
        }
    });
    let watcher = wifi.clone();
    thread::spawn(move || watcher.run_watcher());
    wifi
}
