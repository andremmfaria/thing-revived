//! Outbound wire events. Serde only: no knowledge of the device modules
//! that produce them (that mapping lives in `app::wire_map`). Field shapes
//! here are the contract with the frontend; `config/protocol/controller/*.json`
//! pins them byte for byte.

use serde::Serialize;

/// Wire protocol for the settings-menu backend extension
/// (docs/research/settings-menu-feasibility.md §4/§5/§7/§9). This is the
/// contract the frontend menu UI is built against - field shapes here are
/// the source of truth, not any example quoted in the design doc.
#[derive(Serialize, Clone, Debug)]
#[serde(tag = "type")]
pub enum BridgeEvent {
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
pub struct WifiNetworkWire {
    pub ssid: String,
    pub signal_pct: u8,
    pub signal_dbm: i32,
    pub security: String,
    pub supported: bool,
}

#[derive(Serialize, Clone, Debug)]
pub struct WifiSavedWire {
    pub ssid: String,
    pub current: bool,
}

/// Wire shape for one entry of `BridgeEvent::BtPairedList`. A thin
/// serializable mirror of `bluetooth::PairedDevice` - kept as a separate
/// struct here (not a `derive(Serialize)` on `bluetooth::PairedDevice`
/// itself) for the same reason `BtDeviceFound` doesn't reuse
/// `bluetooth::DiscoveredDevice` directly: `bluetooth.rs`'s own doc
/// comment states it deliberately has no WS/serde dependency, `app::wire_map` is
/// where the D-Bus-facing data model meets the wire format.
#[derive(Serialize, Clone, Debug)]
pub struct PairedDeviceWire {
    pub address: String,
    pub name: Option<String>,
    pub connected: bool,
}
