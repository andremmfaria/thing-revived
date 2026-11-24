//! Event fan-out: the client registry plus a bounded queue per client.
//! `EventHub` implements `EventSink`, so services only know the trait.

use crate::ports::EventSink;
use crate::protocol::BridgeEvent;
use crate::sync_util::lock;
use log::{debug, error, info, trace, warn};
use std::net::TcpStream;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError};
use std::sync::Mutex;

/// Identifies one accepted WebSocket connection for its whole life. Ids are
/// never reused within a process, so a stale id can only miss, never hit
/// another client.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ConnId(pub u64);

/// Sends an event to exactly one connection (a reply to the requester).
/// Implemented by `EventHub` and, in tests, by `RecordingSink`. A reply
/// must never fall back to a broadcast when the connection is gone.
pub trait TargetedSink: Send + Sync {
    /// Returns true only when the event was queued for that connection. A
    /// caller that hands out a secret must not treat `false` as delivered.
    fn emit_to(&self, conn: ConnId, e: BridgeEvent) -> bool;
}

struct ClientSlot {
    id: ConnId,
    tx: SyncSender<String>,
    /// A handle on the client's socket, used only to force-close it when the
    /// client is evicted. That unblocks its writer thread (stuck in a send)
    /// and its reader thread. `None` in tests that have no socket.
    stream: Option<TcpStream>,
}

/// How much of an event goes to the journal (flash-backed on the device).
enum LogPolicy {
    /// High-frequency traffic (knob ticks, button edges, the Wi-Fi status
    /// poll): trace level only.
    Silent,
    /// Type only: the payload is a secret or account data (pairing
    /// passkeys, every `user_*` event). The journal is readable by `adm`.
    TypeOnly,
    /// The whole JSON line. Only for events that carry no secret.
    Full,
}

/// Exhaustive on purpose (no wildcard arm): adding a `BridgeEvent` variant
/// does not compile until its logging is decided here. A new `user_*`,
/// `group_*` or `ssh_key_*` family member must be `TypeOnly`.
fn log_policy(msg: &BridgeEvent) -> LogPolicy {
    use BridgeEvent as E;
    match msg {
        E::Knob { .. } | E::Button { .. } | E::WifiStatus { .. } => LogPolicy::Silent,
        E::BtPairingConfirmRequest { .. } | E::BtPairingPasskeyDisplay { .. } => LogPolicy::TypeOnly,
        // Every user_*, group_* and ssh_key_* event is type only; the two that
        // carry a password (`user_bootstrap_secret`, `user_secret`) above all.
        E::UserBootstrap { .. }
        | E::UserBootstrapSecret { .. }
        | E::UserError { .. }
        | E::UserSession { .. }
        | E::UserList { .. }
        | E::UserResult { .. }
        | E::UserSecret { .. }
        | E::GroupList { .. }
        | E::SshKeyList { .. } => LogPolicy::TypeOnly,
        E::BtPower { .. }
        | E::BtError { .. }
        | E::BtScanStarted { .. }
        | E::BtScanStopped { .. }
        | E::BtDeviceFound { .. }
        | E::BtPairResult { .. }
        | E::BtPairedList { .. }
        | E::Brightness { .. }
        | E::Timezone { .. }
        | E::TimezoneList { .. }
        | E::SettingsError { .. }
        | E::UsbRole { .. }
        | E::UsbRoleError { .. }
        | E::UsbRoleRebooting { .. }
        // `debug_state`/`debug_error` carry no secret, just two booleans,
        // two build-default flags, and a stable blocked code: full logging
        // matches usb_role's own events.
        | E::DebugState { .. }
        | E::DebugError { .. }
        // The device settings events carry four setting values and a stable
        // code, never a secret: full logging like the families above.
        | E::DeviceSettings { .. }
        | E::DeviceSettingsError { .. }
        | E::DeviceRebooting { .. }
        | E::WifiAvailable { .. }
        | E::WifiScanStarted { .. }
        | E::WifiScanResults { .. }
        | E::WifiConnecting { .. }
        | E::WifiConnectResult { .. }
        | E::WifiSavedList { .. }
        | E::WifiError { .. } => LogPolicy::Full,
    }
}

/// Logs one outbound event according to `log_policy`. `json` is only ever
/// written for `Silent` (trace) and `Full`.
fn log_event(msg: &BridgeEvent, json: &str) {
    match log_policy(msg) {
        LogPolicy::Silent => trace!("{json}"),
        LogPolicy::TypeOnly => {
            // The type tag is read from the serialized text, never from a
            // payload field, so nothing but the tag can reach the journal.
            let ty = serde_json::from_str::<serde_json::Value>(json)
                .ok()
                .and_then(|v| v["type"].as_str().map(str::to_owned));
            info!("{} (payload not logged)", ty.unwrap_or_default());
        }
        LogPolicy::Full => info!("{json}"),
    }
}

#[derive(Default)]
pub struct EventHub {
    clients: Mutex<Vec<ClientSlot>>,
    next_id: AtomicU64,
}

impl EventHub {
    pub fn new() -> EventHub {
        EventHub::default()
    }

    /// Adds a client's outbound queue and returns its connection id.
    /// `stream` is the handle used to close the socket if the client is
    /// evicted.
    pub fn register(&self, tx: SyncSender<String>, stream: Option<TcpStream>) -> ConnId {
        let id = ConnId(self.next_id.fetch_add(1, Ordering::Relaxed) + 1);
        lock(&self.clients).push(ClientSlot { id, tx, stream });
        id
    }

    /// Drops a client's slot after its reader loop ended. Idempotent.
    pub fn unregister(&self, conn: ConnId) {
        lock(&self.clients).retain(|slot| slot.id != conn);
    }

    pub fn client_count(&self) -> usize {
        lock(&self.clients).len()
    }

    fn serialize(msg: &BridgeEvent) -> Option<String> {
        match serde_json::to_string(msg) {
            Ok(j) => Some(j),
            Err(e) => {
                error!("failed to serialize event: {e}");
                None
            }
        }
    }

    /// Closes the sockets of evicted clients, outside the client lock.
    fn close(evicted: Vec<ClientSlot>) {
        for slot in evicted {
            if let Some(stream) = slot.stream {
                let _ = stream.shutdown(std::net::Shutdown::Both);
            }
        }
    }
}

impl EventSink for EventHub {
    fn emit(&self, msg: BridgeEvent) {
        let Some(json) = EventHub::serialize(&msg) else { return };
        log_event(&msg, &json);
        // try_send never blocks, so the lock is held only for O(clients) cheap
        // pushes. Evicted clients are closed after the lock is released.
        let mut evicted = Vec::new();
        {
            let mut guard = lock(&self.clients);
            let mut i = 0;
            while i < guard.len() {
                match guard[i].tx.try_send(json.clone()) {
                    Ok(()) => i += 1,
                    Err(TrySendError::Full(_)) => {
                        warn!("ui client queue full, disconnecting it");
                        evicted.push(guard.swap_remove(i));
                    }
                    // Writer thread is gone: prune.
                    Err(TrySendError::Disconnected(_)) => evicted.push(guard.swap_remove(i)),
                }
            }
        }
        EventHub::close(evicted);
    }
}

impl TargetedSink for EventHub {
    /// Writes to one client only, with the same log policy and the same
    /// never-block / evict-on-full behaviour as `emit`. An unknown or
    /// already closed connection drops the event: it is never rebroadcast.
    fn emit_to(&self, conn: ConnId, msg: BridgeEvent) -> bool {
        let Some(json) = EventHub::serialize(&msg) else { return false };
        log_event(&msg, &json);
        let mut delivered = false;
        let mut evicted = Vec::new();
        {
            let mut guard = lock(&self.clients);
            let Some(i) = guard.iter().position(|slot| slot.id == conn) else {
                debug!("targeted event for closed connection {} dropped", conn.0);
                return false;
            };
            match guard[i].tx.try_send(json) {
                Ok(()) => delivered = true,
                Err(TrySendError::Full(_)) => {
                    warn!("ui client queue full, disconnecting it");
                    evicted.push(guard.swap_remove(i));
                }
                Err(TrySendError::Disconnected(_)) => evicted.push(guard.swap_remove(i)),
            }
        }
        EventHub::close(evicted);
        delivered
    }
}

/// An `EventSink` that records events, for tests of anything that emits.
#[derive(Default)]
pub struct RecordingSink {
    events: Mutex<Vec<BridgeEvent>>,
    targeted: Mutex<Vec<(ConnId, BridgeEvent)>>,
}

impl RecordingSink {
    pub fn new() -> RecordingSink {
        RecordingSink::default()
    }

    /// Takes everything recorded so far.
    pub fn take(&self) -> Vec<BridgeEvent> {
        std::mem::take(&mut *lock(&self.events))
    }

    /// Takes the targeted replies recorded so far, with their recipient.
    pub fn take_targeted(&self) -> Vec<(ConnId, BridgeEvent)> {
        std::mem::take(&mut *lock(&self.targeted))
    }
}

impl TargetedSink for RecordingSink {
    fn emit_to(&self, conn: ConnId, e: BridgeEvent) -> bool {
        lock(&self.targeted).push((conn, e));
        true
    }
}

impl EventSink for RecordingSink {
    fn emit(&self, e: BridgeEvent) {
        lock(&self.events).push(e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::wifi::{ConnState, StatusInfo};
    use crate::services::wifi::WifiEvent;
    use std::net::TcpListener;
    use std::sync::mpsc::sync_channel;
    use std::sync::Arc;
    use std::thread;
    use std::time::{Duration, Instant};

    fn add_client(hub: &EventHub, cap: usize) -> std::sync::mpsc::Receiver<String> {
        let (tx, rx) = sync_channel::<String>(cap);
        hub.register(tx, None);
        rx
    }

    #[test]
    fn stalled_client_is_evicted_when_its_queue_fills_and_healthy_client_is_unaffected() {
        let hub = EventHub::new();
        let stalled = add_client(&hub, 4);
        let healthy = add_client(&hub, 64);
        let start = Instant::now();
        for i in 0..10 {
            hub.emit(BridgeEvent::Knob { delta: i });
            // The healthy client drains as it goes; the stalled one never reads.
            assert!(healthy.try_recv().is_ok());
        }
        assert!(start.elapsed() < Duration::from_secs(1), "broadcast must never block");
        assert_eq!(hub.client_count(), 1, "stalled client evicted, healthy kept");
        // Evicted client got exactly its queue capacity, then nothing more.
        assert_eq!(stalled.try_iter().count(), 4);
        assert!(matches!(stalled.try_recv(), Err(std::sync::mpsc::TryRecvError::Disconnected)));
    }

    #[test]
    fn dead_client_is_pruned() {
        let hub = EventHub::new();
        drop(add_client(&hub, 4));
        hub.emit(BridgeEvent::Knob { delta: 1 });
        assert_eq!(hub.client_count(), 0);
    }

    #[test]
    fn eviction_closes_the_client_socket() {
        use std::io::Read;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut peer = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server_side, _) = listener.accept().unwrap();
        let hub = EventHub::new();
        let (tx, _rx) = sync_channel::<String>(1);
        hub.register(tx, Some(server_side));
        hub.emit(BridgeEvent::Knob { delta: 1 });
        hub.emit(BridgeEvent::Knob { delta: 2 }); // queue (cap 1) full: evict
        assert_eq!(hub.client_count(), 0);
        let mut buf = [0u8; 1];
        assert_eq!(peer.read(&mut buf).unwrap(), 0, "peer sees EOF after shutdown");
    }

    #[test]
    fn broadcast_survives_a_poisoned_client_list() {
        let hub = Arc::new(EventHub::new());
        let h2 = hub.clone();
        let _ = thread::spawn(move || {
            let _g = h2.clients.lock().unwrap();
            panic!("poison");
        })
        .join();
        let rx = add_client(&hub, 4);
        hub.emit(BridgeEvent::Knob { delta: 1 });
        assert_eq!(rx.try_recv().unwrap(), r#"{"type":"knob","delta":1}"#);
    }

    fn secret_event() -> BridgeEvent {
        BridgeEvent::UserBootstrapSecret {
            username: "kiosk".into(),
            password: serde_json::from_str(r#""s3cretvalue""#).unwrap(),
        }
    }

    #[test]
    fn emit_to_reaches_only_the_target_and_never_the_others() {
        let hub = EventHub::new();
        let (tx_a, rx_a) = sync_channel::<String>(4);
        let (tx_b, rx_b) = sync_channel::<String>(4);
        let a = hub.register(tx_a, None);
        let b = hub.register(tx_b, None);
        assert_ne!(a, b);
        assert!(hub.emit_to(b, secret_event()), "queued means delivered");
        assert!(rx_a.try_recv().is_err(), "other connection must receive nothing");
        assert!(rx_b.try_recv().unwrap().contains("user_bootstrap_secret"));
        hub.emit(BridgeEvent::BtPower { powered: true });
        assert!(rx_a.try_recv().is_ok() && rx_b.try_recv().is_ok(), "broadcast still reaches everyone");
    }

    #[test]
    fn emit_to_a_closed_connection_is_dropped_not_broadcast() {
        let hub = EventHub::new();
        let (tx_a, rx_a) = sync_channel::<String>(4);
        let (tx_b, _rx_b) = sync_channel::<String>(4);
        let _a = hub.register(tx_a, None);
        let b = hub.register(tx_b, None);
        hub.unregister(b);
        assert!(!hub.emit_to(b, secret_event()), "a closed connection reports not delivered");
        assert!(rx_a.try_recv().is_err());
        assert_eq!(hub.client_count(), 1);
        hub.unregister(b); // idempotent
    }

    #[test]
    fn emit_to_a_full_queue_evicts_only_that_client() {
        let hub = EventHub::new();
        let (tx_a, _rx_a) = sync_channel::<String>(1);
        let (tx_b, _rx_b) = sync_channel::<String>(4);
        let a = hub.register(tx_a, None);
        let _b = hub.register(tx_b, None);
        assert!(hub.emit_to(a, BridgeEvent::BtPower { powered: true }));
        assert!(!hub.emit_to(a, BridgeEvent::BtPower { powered: false }), "a full queue drops it and says so");
        assert_eq!(hub.client_count(), 1);
    }

    #[test]
    fn high_frequency_events_are_not_logged_and_passkeys_are_redacted() {
        assert!(matches!(log_policy(&BridgeEvent::Knob { delta: 1 }), LogPolicy::Silent));
        assert!(matches!(log_policy(&BridgeEvent::Button { code: 1, value: 1 }), LogPolicy::Silent));
        let status = BridgeEvent::from(WifiEvent::Status(StatusInfo {
            state: ConnState::Connected,
            ssid: None,
            ip: None,
            interface: "wlan0".into(),
            signal_pct: None,
            ethernet_up: false,
        }));
        assert!(matches!(log_policy(&status), LogPolicy::Silent));
        let pk = BridgeEvent::BtPairingConfirmRequest { address: "AA".into(), passkey: 123456 };
        assert!(matches!(log_policy(&pk), LogPolicy::TypeOnly));
        assert!(matches!(log_policy(&BridgeEvent::BtPower { powered: true }), LogPolicy::Full));
        assert!(matches!(log_policy(&secret_event()), LogPolicy::TypeOnly));
    }


    #[test]
    fn recording_sink_keeps_events_in_order() {
        let sink = RecordingSink::new();
        sink.emit(BridgeEvent::Knob { delta: 1 });
        sink.emit(BridgeEvent::BtPower { powered: true });
        let got = sink.take();
        assert_eq!(got.len(), 2);
        assert!(matches!(got[1], BridgeEvent::BtPower { powered: true }));
        assert!(sink.take().is_empty());
    }
}
