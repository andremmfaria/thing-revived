//! The WebSocket wire protocol: serde types only, no I/O and no device
//! knowledge. Golden fixtures live in `config/protocol/controller/`.

pub mod command;
pub mod event;

pub use command::{ClientCommand, Secret};
pub use event::{BridgeEvent, PairedDeviceWire, WifiNetworkWire, WifiSavedWire};
