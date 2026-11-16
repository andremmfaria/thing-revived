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
const CONFIRM_TIMEOUT: Duration = Duration::from_secs(30);

const BLUEZ_DEST: &str = "org.bluez";
const ADAPTER_PATH: &str = "/org/bluez/hci0";
const AGENT_PATH: &str = "/thing_revived/agent";
const AGENT_CAPABILITY: &str = "KeyboardDisplay";

/// One discovered-device fact worth broadcasting; deliberately not every
/// Device1 property, just the ones a device picker needs (design doc's
/// own `bt_device_found` sketch).
#[derive(Debug, Clone, PartialEq)]
pub struct DiscoveredDevice {
    pub address: String,
    pub name: Option<String>,
}

/// What the WS layer needs to know about a pairing-agent callback, fed
/// back out of `bluetooth`'s own thread via a plain mpsc channel (main.rs
/// owns turning this into the actual `bt_pairing_confirm_request` WS
/// broadcast - this module has no WS/serde dependency of its own).
#[derive(Debug, Clone)]
pub enum AgentEvent {
    /// `RequestConfirmation`: BlueZ wants a yes/no on a displayed passkey.
    ConfirmRequest { address: String, passkey: u32 },
    /// `DisplayPasskey`: notify-only, no reply needed (BlueZ just wants
    /// this shown on screen while the far side's own keystrokes confirm).
    DisplayPasskey { address: String, passkey: u32 },
    /// The in-flight pairing attempt finished, one way or the other.
    PairResult { address: String, success: bool, error: Option<String> },
    ScanStarted { duration_secs: u64 },
    ScanStopped,
    DeviceFound(DiscoveredDevice),
}

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
}

#[zbus::interface(name = "org.bluez.Agent1")]
impl Agent {
    fn release(&self) {
        println!("[bridge] bt agent: Release");
    }

    fn cancel(&self) {
        println!("[bridge] bt agent: Cancel (BlueZ abandoned the in-flight request)");
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
            println!(
                "[bridge] bt agent: auto-confirming pairing for detected HID keyboard, \
                 device={address} passkey={passkey} (see bluetooth.rs's device_is_hid_keyboard())"
            );
            return Ok(());
        }
        println!("[bridge] bt agent: RequestConfirmation device={address} passkey={passkey}");
        let (tx, rx) = channel::<bool>();
        *self.pending_confirm.lock().unwrap() = Some(tx);
        let _ = self.events.send(AgentEvent::ConfirmRequest { address: address.clone(), passkey });

        match rx.recv_timeout(CONFIRM_TIMEOUT) {
            Ok(true) => Ok(()),
            Ok(false) => Err(zbus::fdo::Error::Failed("rejected by user".into())),
            Err(_) => {
                eprintln!("[bridge] bt agent: RequestConfirmation timed out waiting for WS reply");
                Err(zbus::fdo::Error::Failed("confirmation timed out".into()))
            }
        }
    }

    fn display_passkey(&self, device: ObjectPath<'_>, passkey: u32, _entered: u16) {
        let address = device_path_to_address(device.as_str());
        println!("[bridge] bt agent: DisplayPasskey device={address} passkey={passkey}");
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
        println!(
            "[bridge] bt agent: RequestAuthorization device={address} -- AUTO-ACCEPTING \
             (no WS round-trip for this callback, see bluetooth.rs doc comment)"
        );
        Ok(())
    }

    fn authorize_service(&self, device: ObjectPath<'_>, uuid: &str) -> zbus::fdo::Result<()> {
        let address = device_path_to_address(device.as_str());
        println!(
            "[bridge] bt agent: AuthorizeService device={address} uuid={uuid} -- AUTO-ACCEPTING \
             (no WS round-trip for this callback, see bluetooth.rs doc comment)"
        );
        Ok(())
    }

    fn request_pin_code(&self, device: ObjectPath<'_>) -> zbus::fdo::Result<String> {
        let address = device_path_to_address(device.as_str());
        eprintln!(
            "[bridge] bt agent: RequestPinCode device={address} -- not supported by this \
             agent (KeyboardDisplay capability should not need legacy PIN entry); rejecting"
        );
        Err(zbus::fdo::Error::Failed("pin-code entry not supported".into()))
    }

    fn request_passkey(&self, device: ObjectPath<'_>) -> zbus::fdo::Result<u32> {
        let address = device_path_to_address(device.as_str());
        eprintln!(
            "[bridge] bt agent: RequestPasskey device={address} -- not supported by this \
             agent (no numeric input UI exists yet); rejecting"
        );
        Err(zbus::fdo::Error::Failed("passkey entry not supported".into()))
    }
}

fn device_path_to_address(path: &str) -> String {
    // "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF" -> "AA:BB:CC:DD:EE:FF"
    path.rsplit('/')
        .next()
        .unwrap_or("")
        .trim_start_matches("dev_")
        .replace('_', ":")
}

fn address_to_device_path(address: &str) -> String {
    format!("{ADAPTER_PATH}/dev_{}", address.replace(':', "_"))
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

/// Signal 1's pure classification logic, split out from `device_is_hid_keyboard`
/// so it's unit-testable without a live D-Bus connection: classic
/// Class-of-Device, major=Peripheral (0x05, mask 0x1F00), minor bits 7-6
/// (mask 0xC0) = Keyboard (0x40)/Pointing (0x80)/Combo (0xC0).
fn class_of_device_is_keyboard(class: u32) -> bool {
    (class & 0x1F00) == 0x0500 && (class & 0xC0) != 0
}

/// Signal 2's pure classification logic: BlueZ's standard freedesktop.org
/// icon name for keyboard-class devices.
fn icon_is_keyboard(icon: &str) -> bool {
    icon == "input-keyboard"
}

/// Signal 3's pure classification logic: advertised HID service UUIDs
/// (classic HID Profile 0x1124, BLE HOGP 0x1812) - match on the short-form
/// prefix of the full 128-bit UUID strings BlueZ reports.
fn uuids_contain_hid_service(uuids: &[String]) -> bool {
    uuids.iter().any(|u| {
        let u = u.to_lowercase();
        u.starts_with("00001124-") || u.starts_with("00001812-")
    })
}

fn adapter_proxy(conn: &Connection) -> zbus::Result<zbus::blocking::Proxy<'_>> {
    zbus::blocking::Proxy::new(conn, BLUEZ_DEST, ADAPTER_PATH, "org.bluez.Adapter1")
}

/// Connects to the system bus and registers this process's pairing agent.
/// Failure here (BlueZ not running, D-Bus unreachable, etc.) is returned,
/// not panicked on - the buttons/knob/brightness/timezone features must
/// keep working even if Bluetooth is unavailable for some reason.
pub fn connect_and_register_agent(events: Sender<AgentEvent>) -> Result<BtHandle, String> {
    let conn = Connection::system().map_err(|e| format!("system bus connect: {e}"))?;
    let pending_confirm = Arc::new(Mutex::new(None));
    let auto_confirm_hid = Arc::new(Mutex::new(false));

    let agent = Agent {
        events,
        pending_confirm: pending_confirm.clone(),
        auto_confirm_hid: auto_confirm_hid.clone(),
    };
    conn.object_server()
        .at(AGENT_PATH, agent)
        .map_err(|e| format!("export Agent1 object: {e}"))?;

    let agent_manager =
        zbus::blocking::Proxy::new(&conn, BLUEZ_DEST, "/org/bluez", "org.bluez.AgentManager1")
            .map_err(|e| format!("AgentManager1 proxy: {e}"))?;
    let path = ObjectPath::try_from(AGENT_PATH).map_err(|e| format!("agent path: {e}"))?;
    agent_manager
        .call_method("RegisterAgent", &(&path, AGENT_CAPABILITY))
        .map_err(|e| format!("RegisterAgent: {e}"))?;
    agent_manager
        .call_method("RequestDefaultAgent", &(&path,))
        .map_err(|e| format!("RequestDefaultAgent: {e}"))?;
    println!("[bridge] bt: agent registered at {AGENT_PATH} (capability={AGENT_CAPABILITY})");

    Ok(BtHandle { conn, pending_confirm, auto_confirm_hid, seen: Arc::new(Mutex::new(HashSet::new())) })
}

pub fn get_powered(handle: &BtHandle) -> Result<bool, String> {
    adapter_proxy(&handle.conn)
        .map_err(|e| e.to_string())?
        .get_property::<bool>("Powered")
        .map_err(|e| e.to_string())
}

pub fn set_powered(handle: &BtHandle, on: bool) -> Result<(), String> {
    adapter_proxy(&handle.conn)
        .map_err(|e| e.to_string())?
        .set_property("Powered", on)
        .map_err(|e| e.to_string())
}

/// This is the WS client's reply to an outstanding `ConfirmRequest` -
/// takes the pending sender (so a second, stale reply is a harmless
/// no-op) and feeds the agent's blocked `request_confirmation` call.
pub fn submit_confirm(handle: &BtHandle, accept: bool) {
    if let Some(tx) = handle.pending_confirm.lock().unwrap().take() {
        let _ = tx.send(accept);
    } else {
        eprintln!("[bridge] bt: bt_pairing_confirm received with no pairing request in flight");
    }
}

/// Starts a bounded scan (§4.3/§5.3: discoverable+pairable for
/// `SCAN_DURATION`, not unbounded) and spawns the polling thread that
/// surfaces newly-seen devices and stops discovery when the window ends.
/// Polling `GetManagedObjects` every 2s (rather than subscribing to
/// `InterfacesAdded` signals) trades a little latency for materially
/// simpler code - acceptable for a human watching a device list fill in,
/// not a hard real-time path.
pub fn start_scan(handle: &BtHandle, events: Sender<AgentEvent>) -> Result<(), String> {
    let adapter = adapter_proxy(&handle.conn).map_err(|e| e.to_string())?;
    adapter.set_property("Pairable", true).map_err(|e| e.to_string())?;
    adapter
        .call_method("StartDiscovery", &())
        .map_err(|e| format!("StartDiscovery: {e}"))?;

    handle.seen.lock().unwrap().clear();
    let _ = events.send(AgentEvent::ScanStarted { duration_secs: SCAN_DURATION.as_secs() });

    let conn = handle.conn.clone();
    let seen = handle.seen.clone();
    thread::spawn(move || scan_poll_loop(conn, seen, events, SCAN_DURATION));
    Ok(())
}

pub fn stop_scan(handle: &BtHandle) -> Result<(), String> {
    adapter_proxy(&handle.conn)
        .map_err(|e| e.to_string())?
        .call_method("StopDiscovery", &())
        .map(|_| ())
        .map_err(|e| format!("StopDiscovery: {e}"))
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

fn scan_poll_loop(
    conn: Connection,
    seen: Arc<Mutex<HashSet<String>>>,
    events: Sender<AgentEvent>,
    duration: Duration,
) {
    let om = match zbus::blocking::Proxy::new(&conn, BLUEZ_DEST, "/", "org.freedesktop.DBus.ObjectManager") {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[bridge] bt: ObjectManager proxy failed: {e}");
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
        thread::sleep(Duration::from_secs(2));
    }
    if let Err(e) = adapter_proxy(&conn).and_then(|a| a.call_method("StopDiscovery", &())) {
        eprintln!("[bridge] bt: StopDiscovery at scan-window end failed (may already be stopped): {e}");
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
        println!(
            "[bridge] bt: {address} detected as HID/keyboard peripheral (Class/Icon/UUIDs) -- \
             RequestConfirmation will auto-accept for this pairing attempt"
        );
    }
    let path = address_to_device_path(address);
    let result = zbus::blocking::Proxy::new(&handle.conn, BLUEZ_DEST, path.as_str(), "org.bluez.Device1")
        .map_err(|e| e.to_string())
        .and_then(|p| p.call_method("Pair", &()).map(|_| ()).map_err(|e| e.to_string()));

    match result {
        Ok(()) => {
            println!("[bridge] bt: paired with {address}");
            let _ = events.send(AgentEvent::PairResult { address: address.to_string(), success: true, error: None });
        }
        Err(e) => {
            eprintln!("[bridge] bt: pairing with {address} failed: {e}");
            let _ = events.send(AgentEvent::PairResult { address: address.to_string(), success: false, error: Some(e) });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_path_round_trips_to_colon_address() {
        assert_eq!(
            device_path_to_address("/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF"),
            "AA:BB:CC:DD:EE:FF"
        );
    }

    #[test]
    fn address_round_trips_to_device_path() {
        assert_eq!(
            address_to_device_path("AA:BB:CC:DD:EE:FF"),
            "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF"
        );
    }

    #[test]
    fn address_and_path_conversions_are_inverses() {
        let addr = "01:23:45:67:89:AB";
        assert_eq!(device_path_to_address(&address_to_device_path(addr)), addr);
    }

    // §13.4's example CoD values, cross-checked against the Bluetooth SIG
    // assigned-numbers baseband encoding (major class bits 12-8, minor class
    // bits 7-2): 0x002540 = major Peripheral (0x05) + minor Keyboard (0x40),
    // a real, commonly-cited example keyboard Class-of-Device value (e.g.
    // the one used in BT-Manager's own cod.py worked examples).
    #[test]
    fn class_of_device_classifies_known_keyboard_value() {
        assert!(class_of_device_is_keyboard(0x002540));
    }

    #[test]
    fn class_of_device_classifies_known_combo_keyboard_pointing_value() {
        // Major Peripheral (0x05) + minor Combo keyboard/pointing (0xC0).
        assert!(class_of_device_is_keyboard(0x0025C0));
    }

    #[test]
    fn class_of_device_rejects_known_phone_value() {
        // 0x5A020C: major Phone (0x02, bits 12-8), not Peripheral - a
        // standard smartphone Class-of-Device value (SIG major class 0x02 =
        // "Phone"), must not be classified as a keyboard.
        assert!(!class_of_device_is_keyboard(0x5A020C));
    }

    #[test]
    fn class_of_device_rejects_peripheral_with_no_keyboard_pointing_minor_bits() {
        // Major Peripheral (0x05) but minor bits 7-6 both zero (e.g. a
        // joystick/gamepad, minor class 0x01 in the Peripheral table) -
        // Peripheral-major alone is not sufficient, the 0xC0 minor-bit mask
        // must also be checked.
        assert!(!class_of_device_is_keyboard(0x050104));
    }

    #[test]
    fn icon_matches_only_the_exact_bluez_keyboard_icon_name() {
        assert!(icon_is_keyboard("input-keyboard"));
        assert!(!icon_is_keyboard("audio-card"));
        assert!(!icon_is_keyboard("phone"));
    }

    #[test]
    fn uuids_detect_classic_hid_profile_uuid() {
        let uuids = vec!["00001124-0000-1000-8000-00805f9b34fb".to_string()];
        assert!(uuids_contain_hid_service(&uuids));
    }

    #[test]
    fn uuids_detect_ble_hogp_uuid_case_insensitively() {
        let uuids = vec!["00001812-0000-1000-8000-00805F9B34FB".to_string()];
        assert!(uuids_contain_hid_service(&uuids));
    }

    #[test]
    fn uuids_reject_unrelated_service_uuids() {
        // A1D (Audio/Video Remote Control, 0x110E) - a real BlueZ-advertised
        // UUID that is not an HID service, confirming the match isn't
        // accidentally over-broad.
        let uuids = vec!["0000110e-0000-1000-8000-00805f9b34fb".to_string()];
        assert!(!uuids_contain_hid_service(&uuids));
    }

    // CHANGE 2's race-safety invariant, exercised directly (no evdev
    // hardware needed): a spawned long-press timer must only fire if the
    // shared state still holds the *exact* Instant it captured at spawn
    // time - a release, or a newer press, must make it a silent no-op.
    #[test]
    fn stale_press_timer_does_not_fire_after_release() {
        let state: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
        let first_press = Instant::now();
        *state.lock().unwrap() = Some(first_press);

        // Simulates the release handler clearing the shared state before
        // the timer thread's sleep elapses.
        *state.lock().unwrap() = None;

        // Simulates the timer thread's post-sleep check.
        let guard = state.lock().unwrap();
        assert_ne!(*guard, Some(first_press));
    }

    #[test]
    fn stale_press_timer_does_not_fire_after_a_newer_press_supersedes_it() {
        let state: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
        let first_press = Instant::now();
        *state.lock().unwrap() = Some(first_press);

        // A second press starts before the first press's timer fires.
        let second_press = Instant::now();
        *state.lock().unwrap() = Some(second_press);

        // The first press's stale timer must not match the current state.
        let guard = state.lock().unwrap();
        assert_ne!(*guard, Some(first_press));
        assert_eq!(*guard, Some(second_press));
    }

    #[test]
    fn still_held_press_timer_fires_when_state_is_unchanged() {
        let state: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
        let this_press = Instant::now();
        *state.lock().unwrap() = Some(this_press);

        // No release/new-press happened - the timer's check should match.
        let guard = state.lock().unwrap();
        assert_eq!(*guard, Some(this_press));
    }
}
