//! Central catalogue of the crate's shared types, enums, aliases and traits.
//! `controller_agent::utils::types` is the single discoverable index.
//!
//! # Convention
//!
//! - A type used by two or more modules, and every enum or trait that is part
//!   of a module boundary, is defined here or listed here. Types with no
//!   outside dependencies (the error type, the Bluetooth values) are defined
//!   in this file; everything else keeps its definition next to the code that
//!   owns it and is re-exported below with `pub use`, so the index is complete
//!   without forcing this file to depend on every layer's internals.
//! - A type private to one module and tightly coupled to its implementation
//!   (adapter structs, the Wi-Fi service) stays with its impl and is only
//!   re-exported.
//! - Serde wire structs stay in `protocol/` and are re-exported.
//! - Port traits stay in `ports.rs` (they are the app/adapter boundary) and
//!   are re-exported.
//! - Dependency direction: the definitions in this file depend on `std` only.
//!   The re-exports depend on other modules, but no module imports a
//!   re-export from here except where noted, so no definition depends on a
//!   module that depends back on it. Old paths (`app::error::AppError`,
//!   `domain::bluetooth::AgentEvent`) are kept as re-exports of the
//!   definitions here.
//!
//! # Index
//!
//! | Group | Types | Defined in |
//! |---|---|---|
//! | Errors | `AppError`, `ErrorKind` | here |
//! | Bluetooth values | `AgentEvent`, `DiscoveredDevice`, `PairedDevice`, `AgentEventTx` | here |
//! | Bluetooth raw | `RawDevice` | `domain::bt_paired` |
//! | Domain | `Settings`, `LongPress` | `domain::{settings, longpress}` |
//! | Ports (traits) | `EventSink`, `Backlight`, `SettingsStore`, `TimezoneCtl`, `Restarter`, `BluetoothPort`, `KeyReplay`, `Clock` | `ports` |
//! | Wire protocol | `BridgeEvent`, `ClientCommand`, `Secret`, `BootstrapPhaseWire`, `PairedDeviceWire`, `WifiNetworkWire`, `WifiSavedWire`, `UserWire`, `GroupWire`, `SshKeyWire`, `USER_ERROR_CODES`, `USB_ROLE_ERROR_CODES`, `DEBUG_ERROR_CODES` | `protocol` |
//! | Configuration | `Config`, `Loaded`, `Source`, `ConfigError`, and one `*Config` per section | `utils::config` |
//! | App | `Execution`, `EventHub`, `RecordingSink`, `Router` | `app::{policy, hub, router}` |
//! | Services | `BluetoothService`, `DisplayService`, `TimezoneService`, `SettingsHolder`, `ButtonPolicy`, `SystemClock` | `services` |
//! | Wi-Fi | `WifiEvent`, `Wifi`, `Security`, `ConnState`, `FailKind`, `NetworkInfo`, `SavedNetwork`, `StatusInfo` | `domain::wifi`, `services::wifi` |

use std::fmt;
use std::sync::mpsc::Sender;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// The backend does not exist on this system (for example BlueZ is absent).
    Unavailable,
    /// The request itself was rejected (out of range, unknown name).
    Invalid,
    /// The backend exists but the operation failed.
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppError {
    pub kind: ErrorKind,
    pub message: String,
}

impl AppError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> AppError {
        AppError { kind, message: message.into() }
    }

    pub fn unavailable(message: impl Into<String>) -> AppError {
        AppError::new(ErrorKind::Unavailable, message)
    }

    pub fn invalid(message: impl Into<String>) -> AppError {
        AppError::new(ErrorKind::Invalid, message)
    }

    /// Wraps any displayable backend error as `Failed`. Usable directly in
    /// `map_err(AppError::failed)`.
    pub fn failed(message: impl fmt::Display) -> AppError {
        AppError::new(ErrorKind::Failed, message.to_string())
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for AppError {}

/// Lets legacy `Result<_, String>` call sites keep compiling with `?`.
impl From<AppError> for String {
    fn from(e: AppError) -> String {
        e.message
    }
}

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
/// Sender half of the channel the pairing agent and scan thread use to
/// report asynchronous Bluetooth facts.
pub type AgentEventTx = Sender<AgentEvent>;


// Re-exports: definitions that live next to their owning code.
pub use crate::app::hub::{EventHub, RecordingSink};
pub use crate::app::policy::Execution;
pub use crate::app::router::Router;
pub use crate::domain::bt_paired::RawDevice;
pub use crate::domain::longpress::LongPress;
pub use crate::domain::settings::Settings;
pub use crate::ports::{Backlight, BluetoothPort, Clock, EventSink, KeyReplay, Restarter, SettingsStore, TimezoneCtl};
pub use crate::protocol::{
    BootstrapPhaseWire, BridgeEvent, ClientCommand, GroupWire, PairedDeviceWire, Secret, SshKeyWire, UserWire, WifiNetworkWire, WifiSavedWire,
    USER_ERROR_CODES,
};
pub use crate::services::bluetooth::BluetoothService;
pub use crate::services::display::DisplayService;
pub use crate::services::input::{ButtonPolicy, SystemClock};
pub use crate::services::settings::SettingsHolder;
pub use crate::services::timezone::TimezoneService;
pub use crate::utils::config::{
    BluetoothConfig, Config, ConfigError, DisplayConfig, InputConfig, Loaded, LoggingConfig, ServerConfig, SettingsConfig, Source,
    TimezoneConfig, WifiConfig,
};
pub use crate::domain::wifi::{ConnState, FailKind, NetworkInfo, SavedNetwork, Security, StatusInfo};
pub use crate::services::wifi::{Wifi, WifiEvent};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_re_export_resolves() {
        fn is_trait_object<T: ?Sized>() {}
        is_trait_object::<dyn EventSink>();
        is_trait_object::<dyn Backlight>();
        is_trait_object::<dyn SettingsStore>();
        is_trait_object::<dyn TimezoneCtl>();
        is_trait_object::<dyn Restarter>();
        is_trait_object::<dyn BluetoothPort>();
        is_trait_object::<dyn KeyReplay>();
        is_trait_object::<dyn Clock>();
        fn is_type<T>() {}
        is_type::<(AppError, ErrorKind, AgentEvent, DiscoveredDevice, PairedDevice, AgentEventTx, RawDevice)>();
        is_type::<(Settings, LongPress, BridgeEvent, ClientCommand, Secret)>();
        is_type::<(PairedDeviceWire, WifiNetworkWire, WifiSavedWire, UserWire, GroupWire, SshKeyWire, BootstrapPhaseWire)>();
        assert!(!USER_ERROR_CODES.is_empty());
        is_type::<(Config, Loaded, Source, ConfigError, ServerConfig, LoggingConfig, InputConfig)>();
        is_type::<(BluetoothConfig, WifiConfig, DisplayConfig, TimezoneConfig, SettingsConfig)>();
        is_type::<(Execution, EventHub, RecordingSink, Router)>();
        is_type::<(BluetoothService, DisplayService, TimezoneService, SettingsHolder, ButtonPolicy, SystemClock)>();
        is_type::<(WifiEvent, Wifi, Security, ConnState, FailKind, NetworkInfo, SavedNetwork, StatusInfo)>();
    }

    #[test]
    fn old_paths_are_the_same_types() {
        let a: crate::app::error::AppError = AppError::invalid("x");
        let _: AppError = a;
        let e: crate::domain::bluetooth::AgentEvent = AgentEvent::ScanStopped;
        assert!(matches!(e, AgentEvent::ScanStopped));
    }

    #[test]
    fn constructors_set_the_kind_and_display_is_the_message() {
        assert_eq!(AppError::unavailable("no bluez").kind, ErrorKind::Unavailable);
        assert_eq!(AppError::invalid("bad").kind, ErrorKind::Invalid);
        let e = AppError::failed(std::io::Error::other("boom"));
        assert_eq!(e.kind, ErrorKind::Failed);
        assert_eq!(e.to_string(), "boom");
    }

    #[test]
    fn converts_to_the_legacy_string_form() {
        let s: String = AppError::invalid("brightness 9 out of range").into();
        assert_eq!(s, "brightness 9 out of range");
    }
}
