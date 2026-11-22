//! Event fan-out: the client registry plus a bounded queue per client.
//! `EventHub` implements `EventSink`, so services only know the trait.

use crate::ports::EventSink;
use crate::protocol::BridgeEvent;
use crate::sync_util::lock;
use log::{error, info, trace, warn};
use std::net::TcpStream;
use std::sync::mpsc::{SyncSender, TrySendError};
use std::sync::Mutex;

struct ClientSlot {
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
    /// Type only: the payload is a pairing passkey, treated as a secret.
    TypeOnly,
    Full,
}

fn log_policy(msg: &BridgeEvent) -> LogPolicy {
    match msg {
        BridgeEvent::Knob { .. } | BridgeEvent::Button { .. } | BridgeEvent::WifiStatus { .. } => LogPolicy::Silent,
        BridgeEvent::BtPairingConfirmRequest { .. } | BridgeEvent::BtPairingPasskeyDisplay { .. } => LogPolicy::TypeOnly,
        _ => LogPolicy::Full,
    }
}

#[derive(Default)]
pub struct EventHub {
    clients: Mutex<Vec<ClientSlot>>,
}

impl EventHub {
    pub fn new() -> EventHub {
        EventHub::default()
    }

    /// Adds a client's outbound queue. `stream` is the handle used to close
    /// the socket if the client is evicted.
    pub fn register(&self, tx: SyncSender<String>, stream: Option<TcpStream>) {
        lock(&self.clients).push(ClientSlot { tx, stream });
    }

    pub fn client_count(&self) -> usize {
        lock(&self.clients).len()
    }
}

impl EventSink for EventHub {
    fn emit(&self, msg: BridgeEvent) {
        let json = match serde_json::to_string(&msg) {
            Ok(j) => j,
            Err(e) => {
                error!("failed to serialize event: {e}");
                return;
            }
        };
        match log_policy(&msg) {
            // High-frequency traffic is trace only (and carries no secrets).
            LogPolicy::Silent => trace!("{json}"),
            LogPolicy::TypeOnly => {
                let ty = serde_json::to_value(&msg).ok().and_then(|v| v["type"].as_str().map(str::to_owned));
                info!("{} (payload not logged)", ty.unwrap_or_default());
            }
            LogPolicy::Full => info!("{json}"),
        }
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
        for slot in evicted {
            if let Some(stream) = slot.stream {
                let _ = stream.shutdown(std::net::Shutdown::Both);
            }
        }
    }
}

/// An `EventSink` that records events, for tests of anything that emits.
#[derive(Default)]
pub struct RecordingSink {
    events: Mutex<Vec<BridgeEvent>>,
}

impl RecordingSink {
    pub fn new() -> RecordingSink {
        RecordingSink::default()
    }

    /// Takes everything recorded so far.
    pub fn take(&self) -> Vec<BridgeEvent> {
        std::mem::take(&mut *lock(&self.events))
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
