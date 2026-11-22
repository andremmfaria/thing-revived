//! Bluetooth commands on top of the `BluetoothPort`: every failure, including
//! an absent BlueZ, becomes a `bt_error` reply carrying the request's context.
//! Whenever the paired set might have changed (connect, disconnect, forget, a
//! successful pairing) a fresh `bt_paired_list` is pushed.

use crate::app::error::AppError;
use crate::domain::bluetooth::AgentEvent;
use crate::ports::{BluetoothPort, EventSink};
use crate::protocol::{BridgeEvent, PairedDeviceWire};
use log::warn;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;

pub struct BluetoothService {
    port: Arc<dyn BluetoothPort>,
    sink: Arc<dyn EventSink>,
    events: Sender<AgentEvent>,
}

impl BluetoothService {
    pub fn new(port: Arc<dyn BluetoothPort>, sink: Arc<dyn EventSink>, events: Sender<AgentEvent>) -> BluetoothService {
        BluetoothService { port, sink, events }
    }

    /// The one place a backend error becomes a wire error for the Bluetooth
    /// command family. `Unavailable`, `Invalid` and `Failed` all map to
    /// `bt_error` with the command name as context.
    fn report(&self, context: &str, e: AppError) {
        self.sink.emit(BridgeEvent::BtError { context: context.into(), message: e.message });
    }

    /// Emits `ok` on success or the mapped error.
    fn finish(&self, context: &str, result: Result<(), AppError>, ok: impl FnOnce() -> Option<BridgeEvent>) {
        match result {
            Ok(()) => {
                if let Some(e) = ok() {
                    self.sink.emit(e);
                }
            }
            Err(e) => self.report(context, e),
        }
    }

    pub fn power_get(&self) {
        match self.port.get_powered() {
            Ok(powered) => self.sink.emit(BridgeEvent::BtPower { powered }),
            Err(e) => self.report("bt_power_get", e),
        }
    }

    pub fn set_power(&self, powered: bool) {
        let r = self.port.set_powered(powered);
        self.finish("bt_set_power", r, || Some(BridgeEvent::BtPower { powered }));
    }

    pub fn scan_start(&self) {
        let r = self.port.start_scan(self.events.clone());
        self.finish("bt_scan_start", r, || None);
    }

    pub fn scan_stop(&self) {
        let r = self.port.stop_scan();
        self.finish("bt_scan_stop", r, || None);
    }

    /// Blocks until the pairing resolves; run on a worker thread.
    pub fn pair(&self, address: &str) {
        let r = self.port.pair(address, &self.events);
        self.finish("bt_pair", r, || None);
    }

    pub fn confirm(&self, accept: bool) {
        if let Err(e) = self.port.submit_confirm(accept) {
            warn!("bt_pairing_confirm received but Bluetooth is unavailable: {e}");
        }
    }

    pub fn paired_list(&self) {
        match self.port.list_paired() {
            Ok(devices) => {
                self.sink.emit(BridgeEvent::BtPairedList { devices: devices.into_iter().map(PairedDeviceWire::from).collect() })
            }
            Err(e) => self.report("bt_paired_list_get", e),
        }
    }

    pub fn connect(&self, address: &str) {
        let r = self.port.connect(address);
        self.refresh_after("bt_connect", r);
    }

    pub fn disconnect(&self, address: &str) {
        let r = self.port.disconnect(address);
        self.refresh_after("bt_disconnect", r);
    }

    pub fn forget(&self, address: &str) {
        let r = self.port.forget(address);
        self.refresh_after("bt_forget", r);
    }

    fn refresh_after(&self, context: &str, result: Result<(), AppError>) {
        match result {
            Ok(()) => self.paired_list(),
            Err(e) => self.report(context, e),
        }
    }

    /// Converts agent-side events into wire events until the channel closes,
    /// pushing a fresh paired list after each successful pairing.
    pub fn relay(&self, rx: Receiver<AgentEvent>) {
        for event in rx {
            let refresh = matches!(event, AgentEvent::PairResult { success: true, .. });
            let msg = match event {
                AgentEvent::ConfirmRequest { address, passkey } => BridgeEvent::BtPairingConfirmRequest { address, passkey },
                AgentEvent::DisplayPasskey { address, passkey } => BridgeEvent::BtPairingPasskeyDisplay { address, passkey },
                AgentEvent::PairResult { address, success, error } => BridgeEvent::BtPairResult { address, success, error },
                AgentEvent::ScanStarted { duration_secs } => BridgeEvent::BtScanStarted { duration_secs },
                AgentEvent::ScanStopped => BridgeEvent::BtScanStopped {},
                AgentEvent::DeviceFound(d) => BridgeEvent::BtDeviceFound { address: d.address, name: d.name },
            };
            self.sink.emit(msg);
            if refresh {
                self.paired_list();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::unavailable_bluetooth::UnavailableBluetooth;
    use crate::app::hub::RecordingSink;
    use crate::domain::bluetooth::{DiscoveredDevice, PairedDevice};
    use std::sync::mpsc::channel;
    use std::sync::Mutex;

    /// Records calls; each fallible call fails with `fail` when set.
    #[derive(Default)]
    struct FakePort {
        calls: Mutex<Vec<String>>,
        fail: Mutex<Option<String>>,
        powered: Mutex<bool>,
        paired: Mutex<Vec<PairedDevice>>,
    }

    impl FakePort {
        fn note(&self, c: impl Into<String>) -> Result<(), AppError> {
            self.calls.lock().unwrap().push(c.into());
            match self.fail.lock().unwrap().clone() {
                Some(m) => Err(AppError::failed(m)),
                None => Ok(()),
            }
        }
        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl BluetoothPort for FakePort {
        fn get_powered(&self) -> Result<bool, AppError> {
            self.note("get_powered")?;
            Ok(*self.powered.lock().unwrap())
        }
        fn set_powered(&self, on: bool) -> Result<(), AppError> {
            self.note(format!("set_powered({on})"))
        }
        fn start_scan(&self, events: Sender<AgentEvent>) -> Result<(), AppError> {
            self.note("start_scan")?;
            let _ = events.send(AgentEvent::ScanStarted { duration_secs: 150 });
            Ok(())
        }
        fn stop_scan(&self) -> Result<(), AppError> {
            self.note("stop_scan")
        }
        fn pair(&self, address: &str, events: &Sender<AgentEvent>) -> Result<(), AppError> {
            self.note(format!("pair({address})"))?;
            let _ = events.send(AgentEvent::PairResult { address: address.into(), success: true, error: None });
            Ok(())
        }
        fn submit_confirm(&self, accept: bool) -> Result<(), AppError> {
            self.note(format!("confirm({accept})"))
        }
        fn list_paired(&self) -> Result<Vec<PairedDevice>, AppError> {
            self.note("list_paired")?;
            Ok(self.paired.lock().unwrap().clone())
        }
        fn connect(&self, a: &str) -> Result<(), AppError> {
            self.note(format!("connect({a})"))
        }
        fn disconnect(&self, a: &str) -> Result<(), AppError> {
            self.note(format!("disconnect({a})"))
        }
        fn forget(&self, a: &str) -> Result<(), AppError> {
            self.note(format!("forget({a})"))
        }
    }

    struct Rig {
        port: Arc<FakePort>,
        sink: Arc<RecordingSink>,
        svc: BluetoothService,
        rx: Receiver<AgentEvent>,
    }

    fn rig() -> Rig {
        let port = Arc::new(FakePort::default());
        port.paired.lock().unwrap().push(PairedDevice { address: "AA".into(), name: Some("kb".into()), connected: true });
        let sink = Arc::new(RecordingSink::new());
        let (tx, rx) = channel();
        let svc = BluetoothService::new(port.clone(), sink.clone(), tx);
        Rig { port, sink, svc, rx }
    }

    fn is_paired_list(e: &BridgeEvent) -> bool {
        matches!(e, BridgeEvent::BtPairedList { devices } if devices.len() == 1 && devices[0].address == "AA")
    }

    #[test]
    fn power_get_and_set_report_state() {
        let r = rig();
        *r.port.powered.lock().unwrap() = true;
        r.svc.power_get();
        r.svc.set_power(false);
        let ev = r.sink.take();
        assert!(matches!(ev[0], BridgeEvent::BtPower { powered: true }));
        assert!(matches!(ev[1], BridgeEvent::BtPower { powered: false }));
        assert_eq!(r.port.calls(), ["get_powered", "set_powered(false)"]);
    }

    #[test]
    fn scan_start_hands_the_event_channel_to_the_port_and_stop_is_silent() {
        let r = rig();
        r.svc.scan_start();
        r.svc.scan_stop();
        assert!(r.sink.take().is_empty());
        assert!(matches!(r.rx.try_recv().unwrap(), AgentEvent::ScanStarted { duration_secs: 150 }));
    }

    #[test]
    fn pair_runs_on_the_port_and_success_refreshes_the_paired_list_via_relay() {
        let r = rig();
        r.svc.pair("AA");
        assert!(r.sink.take().is_empty(), "pair itself emits nothing; the agent event does");
        let (tx, rx) = channel();
        tx.send(r.rx.try_recv().unwrap()).unwrap();
        drop(tx);
        r.svc.relay(rx);
        let ev = r.sink.take();
        assert!(matches!(&ev[0], BridgeEvent::BtPairResult { success: true, .. }));
        assert!(is_paired_list(&ev[1]));
    }

    #[test]
    fn failed_pair_result_does_not_refresh_the_list() {
        let r = rig();
        let (tx, rx) = channel();
        tx.send(AgentEvent::PairResult { address: "AA".into(), success: false, error: Some("nope".into()) }).unwrap();
        drop(tx);
        r.svc.relay(rx);
        let ev = r.sink.take();
        assert_eq!(ev.len(), 1);
        assert!(matches!(&ev[0], BridgeEvent::BtPairResult { success: false, .. }));
        assert!(!r.port.calls().contains(&"list_paired".to_string()));
    }

    #[test]
    fn relay_maps_every_agent_event() {
        let r = rig();
        let (tx, rx) = channel();
        for e in [
            AgentEvent::ConfirmRequest { address: "AA".into(), passkey: 1 },
            AgentEvent::DisplayPasskey { address: "AA".into(), passkey: 2 },
            AgentEvent::ScanStarted { duration_secs: 5 },
            AgentEvent::DeviceFound(DiscoveredDevice { address: "BB".into(), name: None }),
            AgentEvent::ScanStopped,
        ] {
            tx.send(e).unwrap();
        }
        drop(tx);
        r.svc.relay(rx);
        let ev = r.sink.take();
        assert!(matches!(ev[0], BridgeEvent::BtPairingConfirmRequest { passkey: 1, .. }));
        assert!(matches!(ev[1], BridgeEvent::BtPairingPasskeyDisplay { passkey: 2, .. }));
        assert!(matches!(ev[2], BridgeEvent::BtScanStarted { duration_secs: 5 }));
        assert!(matches!(&ev[3], BridgeEvent::BtDeviceFound { address, name: None } if address == "BB"));
        assert!(matches!(ev[4], BridgeEvent::BtScanStopped {}));
    }

    #[test]
    fn confirm_goes_to_the_port() {
        let r = rig();
        r.svc.confirm(true);
        assert_eq!(r.port.calls(), ["confirm(true)"]);
        assert!(r.sink.take().is_empty());
    }

    #[test]
    fn paired_list_get_emits_the_list() {
        let r = rig();
        r.svc.paired_list();
        assert!(is_paired_list(&r.sink.take()[0]));
    }

    #[test]
    fn connect_disconnect_forget_each_refresh_the_list_on_success() {
        let r = rig();
        r.svc.connect("AA");
        r.svc.disconnect("AA");
        r.svc.forget("AA");
        let ev = r.sink.take();
        assert_eq!(ev.len(), 3);
        assert!(ev.iter().all(is_paired_list));
        assert_eq!(
            r.port.calls(),
            ["connect(AA)", "list_paired", "disconnect(AA)", "list_paired", "forget(AA)", "list_paired"]
        );
    }

    #[test]
    fn every_failure_is_a_bt_error_with_its_command_context() {
        let r = rig();
        *r.port.fail.lock().unwrap() = Some("boom".into());
        r.svc.power_get();
        r.svc.set_power(true);
        r.svc.scan_start();
        r.svc.scan_stop();
        r.svc.pair("AA");
        r.svc.paired_list();
        r.svc.connect("AA");
        r.svc.disconnect("AA");
        r.svc.forget("AA");
        let got: Vec<(String, String)> = r
            .sink
            .take()
            .into_iter()
            .map(|e| match e {
                BridgeEvent::BtError { context, message } => (context, message),
                other => panic!("unexpected {other:?}"),
            })
            .collect();
        let contexts: Vec<&str> = got.iter().map(|(c, _)| c.as_str()).collect();
        assert_eq!(
            contexts,
            [
                "bt_power_get", "bt_set_power", "bt_scan_start", "bt_scan_stop", "bt_pair", "bt_paired_list_get",
                "bt_connect", "bt_disconnect", "bt_forget"
            ]
        );
        assert!(got.iter().all(|(_, m)| m == "boom"));
    }

    #[test]
    fn absent_bluez_replies_bt_error_unavailable_for_every_command() {
        let sink = Arc::new(RecordingSink::new());
        let (tx, _rx) = channel();
        let svc = BluetoothService::new(Arc::new(UnavailableBluetooth), sink.clone(), tx);
        svc.power_get();
        svc.set_power(true);
        svc.scan_start();
        svc.scan_stop();
        svc.pair("AA");
        svc.confirm(true);
        svc.paired_list();
        svc.connect("AA");
        svc.disconnect("AA");
        svc.forget("AA");
        let ev = sink.take();
        assert_eq!(ev.len(), 9, "confirm only logs");
        assert!(ev.iter().all(|e| matches!(e, BridgeEvent::BtError { message, .. } if message == "Bluetooth unavailable")));
    }
}
