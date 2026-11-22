//! Pure Wi-Fi logic: parsing wpa_supplicant replies and events, validation,
//! connection-state derivation and the connect tracker. No sockets, no files.
//! Wi-Fi uses wpa_supplicant's unix-socket control interface (the same
//! protocol `wpa_cli` speaks), not its D-Bus API: persistence is one
//! documented command (`SAVE_CONFIG`), the text protocol is unit-testable with
//! captured-format fixtures, and no extra crate is needed. See docs/wifi.md.

pub mod events;
pub mod scan;
pub mod status;
pub mod tracker;
pub mod validate;

pub use events::{parse_event, WpaEvent};
pub use scan::{dbm_to_pct, parse_list_networks, parse_scan_results, security_from_flags, NetworkInfo, SavedNetwork, Security};
pub use status::{derive_state, ethernet_is_up, parse_rssi, parse_status, ConnState, NetIf, StatusInfo, WpaStatus};
pub use tracker::{ConnectTracker, FailKind};
pub use validate::{hex_encode, network_settings, set_psk_command, validate_password, validate_ssid};
