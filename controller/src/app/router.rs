//! The single command match: one `ClientCommand` in, service calls and events
//! out. Which thread runs it is decided by `policy::execution`.

use crate::app::policy::{execution, Execution};
use crate::protocol::ClientCommand;
use crate::services::bluetooth::BluetoothService;
use crate::services::display::DisplayService;
use crate::services::timezone::TimezoneService;
use crate::wifi;
use std::sync::Arc;
use std::thread;

pub struct Router {
    bt: Arc<BluetoothService>,
    wifi: Arc<wifi::Wifi>,
    display: DisplayService,
    timezone: TimezoneService,
}

impl Router {
    pub fn new(bt: Arc<BluetoothService>, wifi: Arc<wifi::Wifi>, display: DisplayService, timezone: TimezoneService) -> Router {
        Router { bt, wifi, display, timezone }
    }

    /// Entry point for the reader thread: runs the command inline or hands it
    /// to a worker thread according to the execution policy.
    pub fn handle(self: &Arc<Self>, cmd: ClientCommand) {
        match execution(&cmd) {
            Execution::Inline => self.execute(cmd),
            Execution::Blocking => {
                let router = self.clone();
                thread::spawn(move || router.execute(cmd));
            }
        }
    }

    /// Applies one command on the calling thread. Every arm is self-contained
    /// and never panics on bad or out-of-range input: a malformed or hostile
    /// WS message must degrade to an error reply, not take the daemon down.
    pub fn execute(&self, cmd: ClientCommand) {
        match cmd {
            ClientCommand::BtPowerGet {} => self.bt.power_get(),
            ClientCommand::BtSetPower { powered } => self.bt.set_power(powered),
            ClientCommand::BtScanStart {} => self.bt.scan_start(),
            ClientCommand::BtScanStop {} => self.bt.scan_stop(),
            ClientCommand::BtPair { address } => self.bt.pair(&address),
            ClientCommand::BtPairingConfirm { accept } => self.bt.confirm(accept),
            ClientCommand::BtPairedListGet {} => self.bt.paired_list(),
            ClientCommand::BtConnect { address } => self.bt.connect(&address),
            ClientCommand::BtDisconnect { address } => self.bt.disconnect(&address),
            ClientCommand::BtForget { address } => self.bt.forget(&address),
            ClientCommand::BrightnessGet {} => self.display.get(),
            ClientCommand::BrightnessSet { value } => self.display.set(value),
            ClientCommand::TimezoneGet {} => self.timezone.get(),
            ClientCommand::TimezoneSet { value } => self.timezone.set(value),
            ClientCommand::TimezoneListGet {} => self.timezone.list(),
            ClientCommand::WifiGet {} => self.wifi.announce(),
            ClientCommand::WifiScan {} => self.wifi.scan(),
            ClientCommand::WifiConnect { ssid, password } => self.wifi.connect(ssid, password.expose().to_string()),
            ClientCommand::WifiConnectSaved { ssid } => self.wifi.connect_saved(ssid),
            ClientCommand::WifiDisconnect {} => self.wifi.disconnect(),
            ClientCommand::WifiSavedListGet {} => self.wifi.list_saved(),
            ClientCommand::WifiForget { ssid } => self.wifi.forget(&ssid),
        }
    }
}
