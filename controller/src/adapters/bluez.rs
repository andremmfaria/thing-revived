//! Bluetooth power/scan/pairing over BlueZ's D-Bus API. See
//! docs/research/settings-menu-feasibility.md §4 and
//! docs/research/bt-peripheral-pairing-workflow.md §4 (agent mechanics).
//!
//! D-Bus client choice: `zbus` (blocking API), not `busctl`/`dbus-send`
//! subprocesses - see the doc comment on `register_agent()` for why a
//! subprocess tool can't do the one thing that actually requires a real
//! D-Bus library here (exporting an `org.bluez.Agent1` object BlueZ calls
//! *into*), and why that same connection is then reused for the simpler
//! client-side calls (Adapter1 property get/set, Device1.Pair()) rather
//! than mixing in a second mechanism for those: one dependency, one
//! connection, one error-handling style, matching both research docs'
//! own recommendation (settings-menu-feasibility.md §9.1,
//! bt-peripheral-pairing-workflow.md §4.2).

use log::{error, info, warn};
use crate::app::error::AppError;
use crate::domain::bluetooth::{AgentEvent, DiscoveredDevice, PairedDevice, ADAPTER_PATH};
use crate::domain::bt_classify::{
    address_to_device_path, class_of_device_is_keyboard, device_path_to_address, icon_is_keyboard,
    uuids_contain_hid_service,
};
use crate::domain::bt_paired::{extract_paired_devices, RawDevice};
use crate::ports::BluetoothPort;
use crate::utils::config::BluetoothConfig;
use std::collections::HashSet;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use zbus::blocking::Connection;
use zbus::zvariant::{ObjectPath, OwnedValue};

/// BlueZ's own documented default (`Adapter1.DiscoverableTimeout`) and the
/// pairing-workflow doc's recommended bound (§5.3: "120-180s is BlueZ's
/// own documented convention, not an invented number") - not unbounded
/// discoverable/pairable per the design doc's explicit instruction.
pub const SCAN_DURATION: Duration = Duration::from_secs(150);

/// How long `RequestConfirmation`/`RequestAuthorization` block waiting for
/// the WS client's reply before giving up and rejecting - bounded so a
/// client that never responds (closed tab, etc.) can't wedge BlueZ's
/// agent call forever.
pub const CONFIRM_TIMEOUT: Duration = Duration::from_secs(30);

/// Delay between `GetManagedObjects` polls while scanning. Defaults for all
/// three durations live in `[bluetooth]` config; these constants are the
/// historical values the config defaults must keep matching.
pub const SCAN_POLL_INTERVAL: Duration = Duration::from_secs(2);

const BLUEZ_DEST: &str = "org.bluez";
const AGENT_PATH: &str = "/thing_revived/agent";
const AGENT_CAPABILITY: &str = "KeyboardDisplay";

/// Shared handle the WS layer holds: the open system-bus connection plus
/// the one in-flight confirmation channel (at most one pairing attempt is
/// ever active at a time in this design, matching the menu's own
/// one-screen-at-a-time model).
pub struct BtHandle {
    pub conn: Connection,
    pending_confirm: Arc<Mutex<Option<Sender<bool>>>>,
    /// Set by `pair()` right before every `Device1.Pair()` call, per
    /// docs/research/settings-menu-feasibility.md §13.5: true iff the target
    /// device was just detected as an HID keyboard/peripheral (via
    /// `device_is_hid_keyboard()`), in which case `Agent::request_confirmation`
    /// auto-accepts instead of round-tripping to the WS client - a keyboard has
    /// no way to satisfy an on-screen yes/no prompt. Re-evaluated at the top of
    /// every `pair()` call for every address, so a prior keyboard pairing can
    /// never leak `true` into a later phone/speaker pairing attempt.
    auto_confirm_hid: Arc<Mutex<bool>>,
    seen: Arc<Mutex<HashSet<String>>>,
    scan_duration: Duration,
    scan_poll_interval: Duration,
}

/// The exported `org.bluez.Agent1` object. BlueZ calls these methods
/// directly (over D-Bus, into this process) once registered - this is
/// the half of the agent protocol no subprocess CLI tool can implement,
/// since replying to a specific incoming method call needs a library
/// that tracks D-Bus message serials, not a one-shot client invocation.
struct Agent {
    events: Sender<AgentEvent>,
    pending_confirm: Arc<Mutex<Option<Sender<bool>>>>,
    auto_confirm_hid: Arc<Mutex<bool>>,
    confirm_timeout: Duration,
}

#[zbus::interface(name = "org.bluez.Agent1")]
impl Agent {
    fn release(&self) {
        info!("bt agent: Release");
    }

    fn cancel(&self) {
        info!("bt agent: Cancel (BlueZ abandoned the in-flight request)");
        // Drop any waiting confirmation sender so a stale reply never
        // lands on a request that's no longer live.
        *self.pending_confirm.lock().unwrap() = None;
    }

    /// The real round-trip: broadcast the request over WS, then block
    /// (this is zbus's *blocking* API, running on its own worker thread -
    /// blocking here is the intended usage) until the WS client answers
    /// via `bt_pairing_confirm`, or `CONFIRM_TIMEOUT` elapses.
    fn request_confirmation(
        &self,
        device: ObjectPath<'_>,
        passkey: u32,
    ) -> zbus::fdo::Result<()> {
        let address = device_path_to_address(device.as_str());
        if *self.auto_confirm_hid.lock().unwrap() {
            info!(
                "bt agent: auto-confirming pairing for detected HID keyboard, \
                 device={address} (see bluetooth.rs's device_is_hid_keyboard())"
            );
            return Ok(());
        }
        info!("bt agent: RequestConfirmation device={address}");
        let (tx, rx) = channel::<bool>();
        *self.pending_confirm.lock().unwrap() = Some(tx);
        let _ = self.events.send(AgentEvent::ConfirmRequest { address: address.clone(), passkey });

        match rx.recv_timeout(self.confirm_timeout) {
            Ok(true) => Ok(()),
            Ok(false) => Err(zbus::fdo::Error::Failed("rejected by user".into())),
            Err(_) => {
                warn!("bt agent: RequestConfirmation timed out waiting for WS reply");
                Err(zbus::fdo::Error::Failed("confirmation timed out".into()))
            }
        }
    }

    fn display_passkey(&self, device: ObjectPath<'_>, passkey: u32, _entered: u16) {
        let address = device_path_to_address(device.as_str());
        info!("bt agent: DisplayPasskey device={address}");
        let _ = self.events.send(AgentEvent::DisplayPasskey { address, passkey });
    }

    /// Corner cut, stated plainly (per the task brief's explicit allowance):
    /// `RequestAuthorization`/`AuthorizeService` auto-accept rather than
    /// getting their own WS round-trip. These fire for already-paired/
    /// trusted-device reconnects and service-level authorization, not the
    /// main first-time pairing confirmation flow (that's
    /// `request_confirmation` above, which DOES get the real round-trip) -
    /// auto-accepting here is lower-stakes, but it is a real shortcut, not
    /// a hypothetical one, logged loudly so it's visible in the unit's
    /// journal if it ever matters.
    fn request_authorization(&self, device: ObjectPath<'_>) -> zbus::fdo::Result<()> {
        let address = device_path_to_address(device.as_str());
        info!(
            "bt agent: RequestAuthorization device={address} -- AUTO-ACCEPTING \
             (no WS round-trip for this callback, see bluetooth.rs doc comment)"
        );
        Ok(())
    }

    fn authorize_service(&self, device: ObjectPath<'_>, uuid: &str) -> zbus::fdo::Result<()> {
        let address = device_path_to_address(device.as_str());
        info!(
            "bt agent: AuthorizeService device={address} uuid={uuid} -- AUTO-ACCEPTING \
             (no WS round-trip for this callback, see bluetooth.rs doc comment)"
        );
        Ok(())
    }

    fn request_pin_code(&self, device: ObjectPath<'_>) -> zbus::fdo::Result<String> {
        let address = device_path_to_address(device.as_str());
        warn!(
            "bt agent: RequestPinCode device={address} -- not supported by this \
             agent (KeyboardDisplay capability should not need legacy PIN entry); rejecting"
        );
        Err(zbus::fdo::Error::Failed("pin-code entry not supported".into()))
    }

    fn request_passkey(&self, device: ObjectPath<'_>) -> zbus::fdo::Result<u32> {
        let address = device_path_to_address(device.as_str());
        warn!(
            "bt agent: RequestPasskey device={address} -- not supported by this \
             agent (no numeric input UI exists yet); rejecting"
        );
        Err(zbus::fdo::Error::Failed("passkey entry not supported".into()))
    }
}

/// Detects whether `address` is an HID keyboard/peripheral, per
/// docs/research/settings-menu-feasibility.md §13.4/§13.5: OR of three
/// independent `Device1` signals (Class-of-Device, Icon, advertised HID
/// UUIDs), any one hit counting as a match since none alone is universally
/// populated (`Class`/`Icon` are optional per BlueZ's own doc; BLE-only
/// devices commonly omit `Class`). Every branch degrades to `false` on a
/// missing/unreadable property rather than erroring, matching this file's
/// existing "never panic on a D-Bus shrug" convention.
fn device_is_hid_keyboard(conn: &Connection, address: &str) -> bool {
    let path = address_to_device_path(address);
    let Ok(proxy) = zbus::blocking::Proxy::new(conn, BLUEZ_DEST, path.as_str(), "org.bluez.Device1")
    else {
        return false;
    };

    // Signal 1: classic Class-of-Device.
    if let Ok(class) = proxy.get_property::<u32>("Class") {
        if class_of_device_is_keyboard(class) {
            return true;
        }
    }
    // Signal 2: BlueZ-derived icon name.
    if let Ok(icon) = proxy.get_property::<String>("Icon") {
        if icon_is_keyboard(&icon) {
            return true;
        }
    }
    // Signal 3: advertised HID service UUIDs.
    if let Ok(uuids) = proxy.get_property::<Vec<String>>("UUIDs") {
        if uuids_contain_hid_service(&uuids) {
            return true;
        }
    }
    false
}

fn adapter_proxy(conn: &Connection) -> zbus::Result<zbus::blocking::Proxy<'_>> {
    zbus::blocking::Proxy::new(conn, BLUEZ_DEST, ADAPTER_PATH, "org.bluez.Adapter1")
}

/// Connects to the system bus and registers this process's pairing agent.
/// Failure here (BlueZ not running, D-Bus unreachable, etc.) is returned,
/// not panicked on - the buttons/knob/brightness/timezone features must
/// keep working even if Bluetooth is unavailable for some reason.
pub fn connect_and_register_agent(events: Sender<AgentEvent>, cfg: &BluetoothConfig) -> Result<BtHandle, AppError> {
    let conn = Connection::system().map_err(|e| AppError::failed(format!("system bus connect: {e}")))?;
    let pending_confirm = Arc::new(Mutex::new(None));
    let auto_confirm_hid = Arc::new(Mutex::new(false));

    let agent = Agent {
        events,
        pending_confirm: pending_confirm.clone(),
        auto_confirm_hid: auto_confirm_hid.clone(),
        confirm_timeout: Duration::from_secs(cfg.confirm_timeout_secs),
    };
    conn.object_server()
        .at(AGENT_PATH, agent)
        .map_err(|e| AppError::failed(format!("export Agent1 object: {e}")))?;

    let agent_manager =
        zbus::blocking::Proxy::new(&conn, BLUEZ_DEST, "/org/bluez", "org.bluez.AgentManager1")
            .map_err(|e| AppError::failed(format!("AgentManager1 proxy: {e}")))?;
    let path = ObjectPath::try_from(AGENT_PATH).map_err(|e| AppError::failed(format!("agent path: {e}")))?;
    agent_manager
        .call_method("RegisterAgent", &(&path, AGENT_CAPABILITY))
        .map_err(|e| AppError::failed(format!("RegisterAgent: {e}")))?;
    agent_manager
        .call_method("RequestDefaultAgent", &(&path,))
        .map_err(|e| AppError::failed(format!("RequestDefaultAgent: {e}")))?;
    info!("bt: agent registered at {AGENT_PATH} (capability={AGENT_CAPABILITY})");

    Ok(BtHandle {
        conn,
        pending_confirm,
        auto_confirm_hid,
        seen: Arc::new(Mutex::new(HashSet::new())),
        scan_duration: Duration::from_secs(cfg.scan_duration_secs),
        scan_poll_interval: Duration::from_secs(cfg.scan_poll_interval_secs),
    })
}

pub fn get_powered(handle: &BtHandle) -> Result<bool, AppError> {
    adapter_proxy(&handle.conn)
        .map_err(AppError::failed)?
        .get_property::<bool>("Powered")
        .map_err(AppError::failed)
}

pub fn set_powered(handle: &BtHandle, on: bool) -> Result<(), AppError> {
    adapter_proxy(&handle.conn)
        .map_err(AppError::failed)?
        .set_property("Powered", on)
        .map_err(AppError::failed)
}

/// This is the WS client's reply to an outstanding `ConfirmRequest` -
/// takes the pending sender (so a second, stale reply is a harmless
/// no-op) and feeds the agent's blocked `request_confirmation` call.
pub fn submit_confirm(handle: &BtHandle, accept: bool) {
    if let Some(tx) = handle.pending_confirm.lock().unwrap().take() {
        let _ = tx.send(accept);
    } else {
        warn!("bt: bt_pairing_confirm received with no pairing request in flight");
    }
}

/// Starts a bounded scan (§4.3/§5.3: discoverable+pairable for
/// `SCAN_DURATION`, not unbounded) and spawns the polling thread that
/// surfaces newly-seen devices and stops discovery when the window ends.
/// Polling `GetManagedObjects` every 2s (rather than subscribing to
/// `InterfacesAdded` signals) trades a little latency for materially
/// simpler code - acceptable for a human watching a device list fill in,
/// not a hard real-time path.
pub fn start_scan(handle: &BtHandle, events: Sender<AgentEvent>) -> Result<(), AppError> {
    let adapter = adapter_proxy(&handle.conn).map_err(AppError::failed)?;
    adapter.set_property("Pairable", true).map_err(AppError::failed)?;
    adapter
        .call_method("StartDiscovery", &())
        .map_err(|e| AppError::failed(format!("StartDiscovery: {e}")))?;

    handle.seen.lock().unwrap().clear();
    let _ = events.send(AgentEvent::ScanStarted { duration_secs: handle.scan_duration.as_secs() });

    let conn = handle.conn.clone();
    let seen = handle.seen.clone();
    let (duration, interval) = (handle.scan_duration, handle.scan_poll_interval);
    thread::spawn(move || scan_poll_loop(conn, seen, events, duration, interval));
    Ok(())
}

pub fn stop_scan(handle: &BtHandle) -> Result<(), AppError> {
    adapter_proxy(&handle.conn)
        .map_err(AppError::failed)?
        .call_method("StopDiscovery", &())
        .map(|_| ())
        .map_err(|e| AppError::failed(format!("StopDiscovery: {e}")))
}

/// Pulls a plain `String` out of a D-Bus `OwnedValue`, for the two string
/// Device1 properties this module cares about (`Address`, `Name`) -
/// `OwnedValue` has no direct `TryFrom<&OwnedValue> for String` impl in
/// zvariant 4, but it derefs to `Value`, whose `Str` variant does.
fn value_to_string(v: &OwnedValue) -> Option<String> {
    match &**v {
        zbus::zvariant::Value::Str(s) => Some(s.to_string()),
        _ => None,
    }
}

/// Same pull-a-primitive-out-of-`OwnedValue` shape as `value_to_string`,
/// for the two boolean `Device1` properties the paired-devices screen
/// needs (`Paired`, `Connected`).
fn value_to_bool(v: &OwnedValue) -> Option<bool> {
    match &**v {
        zbus::zvariant::Value::Bool(b) => Some(*b),
        _ => None,
    }
}

/// The exact shape `ObjectManager.GetManagedObjects()` returns: object
/// path -> interface name -> property name -> value. Named once so
/// `list_paired_devices()` and its pure-logic half (`extract_paired_devices`,
/// unit-tested below with no D-Bus connection) share one type instead of
/// repeating this nested-generic signature twice.
type ManagedObjects = std::collections::HashMap<
    zbus::zvariant::OwnedObjectPath,
    std::collections::HashMap<String, std::collections::HashMap<String, OwnedValue>>,
>;

/// Reduces the D-Bus object dump to the plain facts the domain filters on.
fn managed_to_raw(objects: &ManagedObjects) -> Vec<RawDevice> {
    objects
        .values()
        .filter_map(|ifaces| ifaces.get("org.bluez.Device1"))
        .map(|dev| RawDevice {
            address: dev.get("Address").and_then(value_to_string),
            name: dev.get("Name").and_then(value_to_string),
            paired: dev.get("Paired").and_then(value_to_bool).unwrap_or(false),
            connected: dev.get("Connected").and_then(value_to_bool).unwrap_or(false),
        })
        .collect()
}

/// Lists every currently-paired/bonded device via BlueZ's root-level
/// `ObjectManager.GetManagedObjects()` - the same object-manager traversal
/// `scan_poll_loop` already uses for newly-discovered devices (§ settings-
/// menu-feasibility.md's reuse-the-existing-pattern guidance), just a
/// one-shot call instead of a polling loop, and filtered to `Paired` rather
/// than "newly seen this scan window".
pub fn list_paired_devices(handle: &BtHandle) -> Result<Vec<PairedDevice>, AppError> {
    let om = zbus::blocking::Proxy::new(&handle.conn, BLUEZ_DEST, "/", "org.freedesktop.DBus.ObjectManager")
        .map_err(|e| AppError::failed(format!("ObjectManager proxy: {e}")))?;
    let reply = om
        .call_method("GetManagedObjects", &())
        .map_err(|e| AppError::failed(format!("GetManagedObjects: {e}")))?;
    let map = reply
        .body()
        .deserialize::<ManagedObjects>()
        .map_err(|e| AppError::failed(format!("deserialize GetManagedObjects reply: {e}")))?;
    Ok(extract_paired_devices(&managed_to_raw(&map)))
}

/// `Device1.Connect()` - reconnects an already-paired/bonded device (e.g.
/// a keyboard that's gone to sleep). Synchronous D-Bus call; callers that
/// care about UI responsiveness should run this off the WS-read thread
/// the same way `pair()` already is (see main.rs's dispatch), since a
/// real reconnect can take a few seconds.
pub fn connect_device(handle: &BtHandle, address: &str) -> Result<(), AppError> {
    let path = address_to_device_path(address);
    zbus::blocking::Proxy::new(&handle.conn, BLUEZ_DEST, path.as_str(), "org.bluez.Device1")
        .map_err(AppError::failed)
        .and_then(|p| p.call_method("Connect", &()).map(|_| ()).map_err(|e| AppError::failed(format!("Connect: {e}"))))
}

/// `Device1.Disconnect()` - drops the active connection without forgetting
/// the pairing/bond.
pub fn disconnect_device(handle: &BtHandle, address: &str) -> Result<(), AppError> {
    let path = address_to_device_path(address);
    zbus::blocking::Proxy::new(&handle.conn, BLUEZ_DEST, path.as_str(), "org.bluez.Device1")
        .map_err(AppError::failed)
        .and_then(|p| p.call_method("Disconnect", &()).map(|_| ()).map_err(|e| AppError::failed(format!("Disconnect: {e}"))))
}

/// `Adapter1.RemoveDevice(device)` - BlueZ's own documented way to forget a
/// device: removes the device object *and* its cached bonding/pairing
/// information in one call (confirmed directly from BlueZ's own
/// `doc/adapter-api.txt`: "This removes the remote device object at the
/// given path. It will remove also the pairing information."), so this is
/// genuinely "forget", not just "disconnect" - no separate unpair call is
/// needed or exists.
pub fn forget_device(handle: &BtHandle, address: &str) -> Result<(), AppError> {
    let path = address_to_device_path(address);
    let path_obj = ObjectPath::try_from(path.as_str()).map_err(|e| AppError::failed(format!("device path: {e}")))?;
    adapter_proxy(&handle.conn)
        .map_err(AppError::failed)?
        .call_method("RemoveDevice", &(&path_obj,))
        .map(|_| ())
        .map_err(|e| AppError::failed(format!("RemoveDevice: {e}")))
}

fn scan_poll_loop(
    conn: Connection,
    seen: Arc<Mutex<HashSet<String>>>,
    events: Sender<AgentEvent>,
    duration: Duration,
    interval: Duration,
) {
    let om = match zbus::blocking::Proxy::new(&conn, BLUEZ_DEST, "/", "org.freedesktop.DBus.ObjectManager") {
        Ok(p) => p,
        Err(e) => {
            error!("bt: ObjectManager proxy failed: {e}");
            return;
        }
    };
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline {
        if let Ok(reply) = om.call_method("GetManagedObjects", &()) {
            if let Ok(map) = reply
                .body()
                .deserialize::<std::collections::HashMap<zbus::zvariant::OwnedObjectPath, std::collections::HashMap<String, std::collections::HashMap<String, OwnedValue>>>>()
            {
                for ifaces in map.values() {
                    if let Some(dev) = ifaces.get("org.bluez.Device1") {
                        if let Some(address) = dev.get("Address").and_then(value_to_string) {
                            let mut seen_guard = seen.lock().unwrap();
                            if seen_guard.insert(address.clone()) {
                                drop(seen_guard);
                                let name = dev.get("Name").and_then(value_to_string);
                                let _ = events.send(AgentEvent::DeviceFound(DiscoveredDevice { address, name }));
                            }
                        }
                    }
                }
            }
        }
        thread::sleep(interval);
    }
    if let Err(e) = adapter_proxy(&conn).and_then(|a| a.call_method("StopDiscovery", &())) {
        warn!("bt: StopDiscovery at scan-window end failed (may already be stopped): {e}");
    }
    let _ = events.send(AgentEvent::ScanStopped);
}

/// Calls `Device1.Pair()`. This is a synchronous D-Bus call that itself
/// blocks until BlueZ's pairing state machine resolves (which, for a
/// `RequestConfirmation` flow, means waiting on THIS process's own agent
/// callback to get its WS round-trip answered) - callers MUST run this on
/// its own thread, never on the thread that also reads incoming WS
/// messages, or the `bt_pairing_confirm` reply could never be delivered
/// (see main.rs's dispatch for where this is spawned).
pub fn pair(handle: &BtHandle, address: &str, events: &Sender<AgentEvent>) {
    // Re-evaluated at the top of every call, for every address, so a prior
    // keyboard pairing's `true` can never leak into a later phone/speaker
    // pairing attempt (see `BtHandle::auto_confirm_hid`'s doc comment).
    let is_hid = device_is_hid_keyboard(&handle.conn, address);
    *handle.auto_confirm_hid.lock().unwrap() = is_hid;
    if is_hid {
        info!(
            "bt: {address} detected as HID/keyboard peripheral (Class/Icon/UUIDs) -- \
             RequestConfirmation will auto-accept for this pairing attempt"
        );
    }
    let path = address_to_device_path(address);
    let result = zbus::blocking::Proxy::new(&handle.conn, BLUEZ_DEST, path.as_str(), "org.bluez.Device1")
        .map_err(|e| e.to_string())
        .and_then(|p| p.call_method("Pair", &()).map(|_| ()).map_err(|e| e.to_string()));

    match result {
        Ok(()) => {
            info!("bt: paired with {address}");
            let _ = events.send(AgentEvent::PairResult { address: address.to_string(), success: true, error: None });
        }
        Err(e) => {
            warn!("bt: pairing with {address} failed: {e}");
            let _ = events.send(AgentEvent::PairResult { address: address.to_string(), success: false, error: Some(e) });
        }
    }
}

/// Production `BluetoothPort`: the BlueZ D-Bus handle behind the port trait.
pub struct BluezBluetooth {
    handle: BtHandle,
}

impl BluezBluetooth {
    pub fn connect(events: Sender<AgentEvent>, cfg: &BluetoothConfig) -> Result<BluezBluetooth, AppError> {
        connect_and_register_agent(events, cfg).map(|handle| BluezBluetooth { handle })
    }
}

impl BluetoothPort for BluezBluetooth {
    fn get_powered(&self) -> Result<bool, AppError> {
        get_powered(&self.handle)
    }
    fn set_powered(&self, on: bool) -> Result<(), AppError> {
        set_powered(&self.handle, on)
    }
    fn start_scan(&self, events: Sender<AgentEvent>) -> Result<(), AppError> {
        start_scan(&self.handle, events)
    }
    fn stop_scan(&self) -> Result<(), AppError> {
        stop_scan(&self.handle)
    }
    fn pair(&self, address: &str, events: &Sender<AgentEvent>) -> Result<(), AppError> {
        pair(&self.handle, address, events);
        Ok(())
    }
    fn submit_confirm(&self, accept: bool) -> Result<(), AppError> {
        submit_confirm(&self.handle, accept);
        Ok(())
    }
    fn list_paired(&self) -> Result<Vec<PairedDevice>, AppError> {
        list_paired_devices(&self.handle)
    }
    fn connect(&self, address: &str) -> Result<(), AppError> {
        connect_device(&self.handle, address)
    }
    fn disconnect(&self, address: &str) -> Result<(), AppError> {
        disconnect_device(&self.handle, address)
    }
    fn forget(&self, address: &str) -> Result<(), AppError> {
        forget_device(&self.handle, address)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_equal_the_historical_constants() {
        let c = BluetoothConfig::default();
        assert_eq!(Duration::from_secs(c.scan_duration_secs), SCAN_DURATION);
        assert_eq!(Duration::from_secs(c.confirm_timeout_secs), CONFIRM_TIMEOUT);
        assert_eq!(Duration::from_secs(c.scan_poll_interval_secs), SCAN_POLL_INTERVAL);
    }

    // --- extract_paired_devices (ObjectManager-dump parsing, no D-Bus needed) ---

    fn fake_managed_objects(
        devices: &[(&str, &str, Option<&str>, bool, bool)],
    ) -> ManagedObjects {
        // Each tuple: (object_path, address, name, paired, connected).
        let mut out = ManagedObjects::new();
        for (path, address, name, paired, connected) in devices {
            let mut props: std::collections::HashMap<String, OwnedValue> = std::collections::HashMap::new();
            props.insert("Address".into(), zbus::zvariant::Value::from(*address).try_to_owned().unwrap());
            if let Some(n) = name {
                props.insert("Name".into(), zbus::zvariant::Value::from(*n).try_to_owned().unwrap());
            }
            props.insert("Paired".into(), zbus::zvariant::Value::from(*paired).try_to_owned().unwrap());
            props.insert("Connected".into(), zbus::zvariant::Value::from(*connected).try_to_owned().unwrap());
            let mut ifaces: std::collections::HashMap<String, std::collections::HashMap<String, OwnedValue>> =
                std::collections::HashMap::new();
            ifaces.insert("org.bluez.Device1".into(), props);
            out.insert(zbus::zvariant::OwnedObjectPath::try_from(*path).unwrap(), ifaces);
        }
        out
    }

    #[test]
    fn managed_objects_reduce_to_raw_devices_and_then_paired_list() {
        let objects = fake_managed_objects(&[
            ("/org/bluez/hci0/dev_BB_BB_BB_BB_BB_BB", "BB:BB:BB:BB:BB:BB", None, false, false),
            ("/org/bluez/hci0/dev_AA_AA_AA_AA_AA_AA", "AA:AA:AA:AA:AA:AA", Some("Keyboard"), true, true),
        ]);
        let paired = extract_paired_devices(&managed_to_raw(&objects));
        assert_eq!(
            paired,
            vec![PairedDevice { address: "AA:AA:AA:AA:AA:AA".into(), name: Some("Keyboard".into()), connected: true }]
        );
    }

    #[test]
    fn non_device1_interfaces_are_never_paired_devices() {
        let mut objects = ManagedObjects::new();
        objects.insert(
            zbus::zvariant::OwnedObjectPath::try_from("/org/bluez/hci0").unwrap(),
            std::collections::HashMap::new(),
        );
        assert!(managed_to_raw(&objects).is_empty());
    }
}
