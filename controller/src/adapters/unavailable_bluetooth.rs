//! Stand-in `BluetoothPort` used when BlueZ or D-Bus was not reachable at
//! startup: every call reports `Unavailable`, so the rest of the daemon
//! (buttons, knob, brightness, timezone) keeps working.

use crate::app::error::AppError;
use crate::domain::bluetooth::{AgentEvent, PairedDevice};
use crate::ports::BluetoothPort;
use std::sync::mpsc::Sender;

pub struct UnavailableBluetooth;

pub const UNAVAILABLE_MESSAGE: &str = "Bluetooth unavailable";

fn unavailable<T>() -> Result<T, AppError> {
    Err(AppError::unavailable(UNAVAILABLE_MESSAGE))
}

impl BluetoothPort for UnavailableBluetooth {
    fn get_powered(&self) -> Result<bool, AppError> {
        unavailable()
    }
    fn set_powered(&self, _on: bool) -> Result<(), AppError> {
        unavailable()
    }
    fn start_scan(&self, _events: Sender<AgentEvent>) -> Result<(), AppError> {
        unavailable()
    }
    fn stop_scan(&self) -> Result<(), AppError> {
        unavailable()
    }
    fn pair(&self, _address: &str, _events: &Sender<AgentEvent>) -> Result<(), AppError> {
        unavailable()
    }
    fn submit_confirm(&self, _accept: bool) -> Result<(), AppError> {
        unavailable()
    }
    fn list_paired(&self) -> Result<Vec<PairedDevice>, AppError> {
        unavailable()
    }
    fn connect(&self, _address: &str) -> Result<(), AppError> {
        unavailable()
    }
    fn disconnect(&self, _address: &str) -> Result<(), AppError> {
        unavailable()
    }
    fn forget(&self, _address: &str) -> Result<(), AppError> {
        unavailable()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::error::ErrorKind;
    use std::sync::mpsc::channel;

    #[test]
    fn every_call_is_unavailable() {
        let p = UnavailableBluetooth;
        let (tx, _rx) = channel();
        let kinds = [
            p.get_powered().err(),
            p.set_powered(true).err(),
            p.start_scan(tx.clone()).err(),
            p.stop_scan().err(),
            p.pair("AA", &tx).err(),
            p.submit_confirm(true).err(),
            p.list_paired().err(),
            p.connect("AA").err(),
            p.disconnect("AA").err(),
            p.forget("AA").err(),
        ];
        for e in kinds {
            let e = e.expect("must fail");
            assert_eq!(e.kind, ErrorKind::Unavailable);
            assert_eq!(e.message, UNAVAILABLE_MESSAGE);
        }
    }
}
