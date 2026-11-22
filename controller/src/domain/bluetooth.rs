//! Bluetooth domain values shared by the port, adapter and service. No
//! D-Bus or serde types: the wire mapping lives in `app::wire_map`.

/// BlueZ adapter object path this device always uses (single radio).
pub const ADAPTER_PATH: &str = "/org/bluez/hci0";

/// One discovered-device fact worth broadcasting; deliberately not every
/// Device1 property, just the ones a device picker needs (design doc's
/// own `bt_device_found` sketch).
#[derive(Debug, Clone, PartialEq)]
pub struct DiscoveredDevice {
    pub address: String,
    pub name: Option<String>,
}

/// One already-paired device's state, for the "Paired devices" management
/// screen (reconnect/forget) - the three `Device1` properties that screen
/// actually needs (`Address`, `Name`, `Connected`), same minimal-fields
/// philosophy as `DiscoveredDevice` above. No `serde` derive here either
/// (this module's own stated no-WS/serde-dependency convention, see the
/// file's top doc comment) - `main.rs` maps this into its own serializable
/// wire shape, exactly as it already does for `DiscoveredDevice` ->
/// `BtDeviceFound`.
#[derive(Debug, Clone, PartialEq)]
pub struct PairedDevice {
    pub address: String,
    pub name: Option<String>,
    pub connected: bool,
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
