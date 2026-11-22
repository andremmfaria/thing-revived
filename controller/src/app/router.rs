//! The single command match: one `ClientCommand` in, service calls and events
//! out. Which thread runs it is decided by `policy::execution`.

use crate::app::hub::{ConnId, TargetedSink};
use crate::app::policy::{execution, Execution};
use crate::protocol::{BridgeEvent, ClientCommand};
use crate::services::bluetooth::BluetoothService;
use crate::services::display::DisplayService;
use crate::services::timezone::TimezoneService;
use crate::services::wifi::Wifi;
use std::sync::{Arc, OnceLock};
use std::thread;

pub struct Router {
    bt: Arc<BluetoothService>,
    wifi: Arc<Wifi>,
    display: DisplayService,
    timezone: TimezoneService,
    /// Where replies to one connection go. Attached once by the WebSocket
    /// server at startup; unset only in tests that do not use it.
    replies: OnceLock<Arc<dyn TargetedSink>>,
}

impl Router {
    pub fn new(bt: Arc<BluetoothService>, wifi: Arc<Wifi>, display: DisplayService, timezone: TimezoneService) -> Router {
        Router { bt, wifi, display, timezone, replies: OnceLock::new() }
    }

    /// Sets the sink for targeted replies. The first call wins.
    pub fn attach_replies(&self, sink: Arc<dyn TargetedSink>) {
        let _ = self.replies.set(sink);
    }

    fn reply(&self, conn: ConnId, e: BridgeEvent) {
        match self.replies.get() {
            Some(sink) => sink.emit_to(conn, e),
            None => log::error!("targeted reply dropped: no reply sink attached"),
        }
    }

    /// Entry point for the reader thread: runs the command inline or hands it
    /// to a worker thread according to the execution policy.
    pub fn handle(self: &Arc<Self>, conn: ConnId, cmd: ClientCommand) {
        match execution(&cmd) {
            Execution::Inline => self.execute(conn, cmd),
            Execution::Blocking => {
                let router = self.clone();
                thread::spawn(move || router.execute(conn, cmd));
            }
        }
    }

    /// Applies one command on the calling thread. Every arm is self-contained
    /// and never panics on bad or out-of-range input: a malformed or hostile
    /// WS message must degrade to an error reply, not take the daemon down.
    pub fn execute(&self, conn: ConnId, cmd: ClientCommand) {
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
            // Placeholder until the bootstrap service lands (T-P1.7b).
            ClientCommand::UserBootstrapGet {} => self.unavailable(conn, "user_bootstrap_get"),
            ClientCommand::UserBootstrapReveal {} => self.unavailable(conn, "user_bootstrap_reveal"),
            ClientCommand::UserBootstrapAck { .. } => self.unavailable(conn, "user_bootstrap_ack"),
        }
    }

    fn unavailable(&self, conn: ConnId, context: &str) {
        self.reply(
            conn,
            BridgeEvent::UserError {
                context: context.into(),
                code: "unavailable".into(),
                message: "User management is not available".into(),
            },
        );
    }
}
