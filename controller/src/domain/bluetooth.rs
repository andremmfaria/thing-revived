//! Bluetooth domain values shared by the port, adapter and service. No
//! D-Bus or serde types: the wire mapping lives in `app::wire_map`.

/// BlueZ adapter object path this device always uses (single radio).
pub const ADAPTER_PATH: &str = "/org/bluez/hci0";

pub use crate::utils::types::{AgentEvent, DiscoveredDevice, PairedDevice};
