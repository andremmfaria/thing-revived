//! Pure logic: no I/O, no serde derive types, no zbus or evdev types (`device_settings` reads JSON through `serde_json::Value` so the boot apply and the controller share one loader).

pub mod accounts;
pub mod bluetooth;
pub mod brightness;
pub mod keys;
pub mod bt_classify;
pub mod bt_paired;
pub mod device_settings;
pub mod longpress;
pub mod settings;
pub mod timezone;
pub mod wifi;
