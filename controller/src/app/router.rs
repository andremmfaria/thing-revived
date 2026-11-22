//! The single command match: one `ClientCommand` in, service calls and events
//! out. Which thread runs it is decided by `policy::execution`.

use crate::app::error::AppError;
use crate::app::policy::{execution, Execution};
use crate::bluetooth;
use crate::ports::EventSink;
use crate::protocol::{BridgeEvent, ClientCommand, PairedDeviceWire};
use crate::services::display::DisplayService;
use crate::services::timezone::TimezoneService;
use crate::wifi;
use log::warn;
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::thread;

pub struct Router {
    sink: Arc<dyn EventSink>,
    /// `None` when BlueZ/D-Bus wasn't reachable at startup: BT commands then
    /// degrade to a `bt_error` reply per request rather than taking down the
    /// rest of the daemon (buttons, knob, brightness, timezone keep working).
    bt: Option<Arc<bluetooth::BtHandle>>,
    bt_events: Sender<bluetooth::AgentEvent>,
    wifi: Arc<wifi::Wifi>,
    display: DisplayService,
    timezone: TimezoneService,
}

impl Router {
    pub fn new(
        sink: Arc<dyn EventSink>,
        bt: Option<Arc<bluetooth::BtHandle>>,
        bt_events: Sender<bluetooth::AgentEvent>,
        wifi: Arc<wifi::Wifi>,
        display: DisplayService,
        timezone: TimezoneService,
    ) -> Router {
        Router { sink, bt, bt_events, wifi, display, timezone }
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
        let sink = &self.sink;
        let bt_unavailable = |context: &str| {
            sink.emit(BridgeEvent::BtError { context: context.into(), message: "Bluetooth unavailable".into() })
        };
        match cmd {
            ClientCommand::BtPowerGet {} => match &self.bt {
                Some(bt) => match bluetooth::get_powered(bt) {
                    Ok(powered) => sink.emit(BridgeEvent::BtPower { powered }),
                    Err(AppError { message, .. }) => sink.emit(BridgeEvent::BtError { context: "bt_power_get".into(), message }),
                },
                None => bt_unavailable("bt_power_get"),
            },
            ClientCommand::BtSetPower { powered } => match &self.bt {
                Some(bt) => match bluetooth::set_powered(bt, powered) {
                    Ok(()) => sink.emit(BridgeEvent::BtPower { powered }),
                    Err(AppError { message, .. }) => sink.emit(BridgeEvent::BtError { context: "bt_set_power".into(), message }),
                },
                None => bt_unavailable("bt_set_power"),
            },
            ClientCommand::BtScanStart {} => match &self.bt {
                Some(bt) => {
                    if let Err(AppError { message, .. }) = bluetooth::start_scan(bt, self.bt_events.clone()) {
                        sink.emit(BridgeEvent::BtError { context: "bt_scan_start".into(), message });
                    }
                }
                None => bt_unavailable("bt_scan_start"),
            },
            ClientCommand::BtScanStop {} => match &self.bt {
                Some(bt) => {
                    if let Err(AppError { message, .. }) = bluetooth::stop_scan(bt) {
                        sink.emit(BridgeEvent::BtError { context: "bt_scan_stop".into(), message });
                    }
                }
                None => bt_unavailable("bt_scan_stop"),
            },
            ClientCommand::BtPair { address } => match &self.bt {
                Some(bt) => bluetooth::pair(bt, &address, &self.bt_events),
                None => bt_unavailable("bt_pair"),
            },
            ClientCommand::BtPairingConfirm { accept } => match &self.bt {
                Some(bt) => bluetooth::submit_confirm(bt, accept),
                None => warn!("bt_pairing_confirm received but Bluetooth is unavailable"),
            },
            ClientCommand::BtPairedListGet {} => match &self.bt {
                Some(bt) => broadcast_paired_list(sink.as_ref(), bt),
                None => bt_unavailable("bt_paired_list_get"),
            },
            ClientCommand::BtConnect { address } => match &self.bt {
                Some(bt) => match bluetooth::connect_device(bt, &address) {
                    Ok(()) => broadcast_paired_list(sink.as_ref(), bt),
                    Err(AppError { message, .. }) => sink.emit(BridgeEvent::BtError { context: "bt_connect".into(), message }),
                },
                None => bt_unavailable("bt_connect"),
            },
            ClientCommand::BtDisconnect { address } => match &self.bt {
                Some(bt) => match bluetooth::disconnect_device(bt, &address) {
                    Ok(()) => broadcast_paired_list(sink.as_ref(), bt),
                    Err(AppError { message, .. }) => sink.emit(BridgeEvent::BtError { context: "bt_disconnect".into(), message }),
                },
                None => bt_unavailable("bt_disconnect"),
            },
            ClientCommand::BtForget { address } => match &self.bt {
                Some(bt) => match bluetooth::forget_device(bt, &address) {
                    Ok(()) => broadcast_paired_list(sink.as_ref(), bt),
                    Err(AppError { message, .. }) => sink.emit(BridgeEvent::BtError { context: "bt_forget".into(), message }),
                },
                None => bt_unavailable("bt_forget"),
            },
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

/// Looks up the current paired-device set and emits it (or a `bt_error` if the
/// lookup itself fails): the one place every "the paired set might have just
/// changed" path funnels through, so `bt_paired_list_get`, a successful
/// connect/disconnect/forget and a successful new pairing all keep the
/// frontend's list in sync via the exact same call.
pub fn broadcast_paired_list(sink: &dyn EventSink, bt: &bluetooth::BtHandle) {
    match bluetooth::list_paired_devices(bt) {
        Ok(devices) => sink.emit(BridgeEvent::BtPairedList { devices: devices.into_iter().map(PairedDeviceWire::from).collect() }),
        Err(AppError { message, .. }) => sink.emit(BridgeEvent::BtError { context: "bt_paired_list_get".into(), message }),
    }
}

/// Converts `bluetooth::AgentEvent`s (D-Bus-callback-originated, no WS
/// knowledge) into wire events, and after a successful new pairing pushes a
/// fresh `bt_paired_list` proactively, without waiting for the frontend to
/// ask. Persists nothing: bonded-device storage is BlueZ's own job (design
/// doc section 8.1). `bt` is `None` when BlueZ wasn't reachable at startup.
pub fn relay_bt_events(sink: Arc<dyn EventSink>, bt: Option<Arc<bluetooth::BtHandle>>, rx: std::sync::mpsc::Receiver<bluetooth::AgentEvent>) {
    use bluetooth::AgentEvent;
    for event in rx {
        let refresh_paired_list = matches!(event, AgentEvent::PairResult { success: true, .. });
        let msg = match event {
            AgentEvent::ConfirmRequest { address, passkey } => BridgeEvent::BtPairingConfirmRequest { address, passkey },
            AgentEvent::DisplayPasskey { address, passkey } => BridgeEvent::BtPairingPasskeyDisplay { address, passkey },
            AgentEvent::PairResult { address, success, error } => BridgeEvent::BtPairResult { address, success, error },
            AgentEvent::ScanStarted { duration_secs } => BridgeEvent::BtScanStarted { duration_secs },
            AgentEvent::ScanStopped => BridgeEvent::BtScanStopped {},
            AgentEvent::DeviceFound(d) => BridgeEvent::BtDeviceFound { address: d.address, name: d.name },
        };
        sink.emit(msg);
        if refresh_paired_list {
            if let Some(bt) = &bt {
                broadcast_paired_list(sink.as_ref(), bt);
            }
        }
    }
}
