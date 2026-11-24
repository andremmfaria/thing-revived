//! The WebSocket wire protocol: serde types only, no I/O and no device
//! knowledge. Golden fixtures live in `config/protocol/controller/`.

pub mod command;
pub mod event;

pub use command::{ClientCommand, Secret};
pub use event::{
    BootstrapPhaseWire, BridgeEvent, GroupWire, PairedDeviceWire, SshKeyWire, UserWire, WifiNetworkWire, WifiSavedWire, USB_ROLE_ERROR_CODES, USER_ERROR_CODES,
};
