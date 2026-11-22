//! Client fanout with bounded per-client queues.
//!
//! The PTY reader must never block on, or accumulate unbounded memory for, a
//! slow client. Each client gets a fixed-capacity queue; if it is full the
//! client is dropped (its sender is removed, which makes the writer thread
//! see a disconnect and close the socket) instead of stalling the reader.

use crate::protocol::{self, ServerMsg};
use crate::utils::types::Msg;
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};

/// Fanout point for all connected clients. Cloning shares the same client
/// list.
#[derive(Clone)]
pub struct ClientHub {
    clients: Arc<Mutex<Vec<SyncSender<Msg>>>>,
    capacity: usize,
}

impl ClientHub {
    /// `capacity` is the number of messages queued per client before it is
    /// considered stalled. Output messages carry at most one read buffer, so
    /// this bounds a stalled client to roughly capacity x read size (a few
    /// times that worst case with JSON escaping).
    pub fn new(capacity: usize) -> Self {
        ClientHub { clients: Arc::new(Mutex::new(Vec::new())), capacity }
    }

    fn guard(&self) -> std::sync::MutexGuard<'_, Vec<SyncSender<Msg>>> {
        self.clients.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Registers a client and returns its receiving end.
    pub fn register(&self) -> Receiver<Msg> {
        let (tx, rx) = sync_channel::<Msg>(self.capacity);
        self.guard().push(tx);
        rx
    }

    pub fn client_count(&self) -> usize {
        self.guard().len()
    }

    /// Queues `json` for every client without blocking. Clients whose queue
    /// is full or whose receiver is gone are removed. Returns how many were
    /// dropped for being full. Only try_send (no I/O, no waiting) runs under
    /// the lock.
    pub fn broadcast_str(&self, json: &str) -> usize {
        let msg: Msg = Arc::from(json);
        let mut dropped_full = 0;
        self.guard().retain(|tx| match tx.try_send(msg.clone()) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                dropped_full += 1;
                false
            }
            Err(TrySendError::Disconnected(_)) => false,
        });
        dropped_full
    }

    /// Serializes `msg` and queues it for every client. Logs (never blocks)
    /// when stalled clients had to be dropped. Returns how many were dropped.
    pub fn broadcast(&self, msg: &ServerMsg) -> usize {
        let dropped = self.broadcast_str(&protocol::encode(msg));
        if dropped > 0 {
            log::warn!("dropped {dropped} stalled client(s): queue full");
        }
        dropped
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn stalled_client_is_dropped_without_blocking() {
        let clients = ClientHub::new(4);
        let _stalled = clients.register(); // never read
        let healthy = clients.register();
        let start = Instant::now();
        let mut total_dropped = 0;
        for i in 0..50 {
            total_dropped += clients.broadcast_str(&format!("m{i}"));
            // The healthy client keeps draining.
            while healthy.try_recv().is_ok() {}
        }
        assert!(start.elapsed() < Duration::from_secs(1));
        assert_eq!(total_dropped, 1);
        assert_eq!(clients.client_count(), 1);
    }

    #[test]
    fn queue_memory_is_bounded_by_capacity() {
        let clients = ClientHub::new(8);
        let rx = clients.register();
        for i in 0..1000 {
            clients.broadcast_str(&format!("{i}"));
        }
        // Only the first 8 were queued; the client was then dropped.
        assert_eq!(rx.try_iter().count(), 8);
        assert_eq!(clients.client_count(), 0);
        // Sender dropped, so a writer thread sees Disconnected after draining.
        assert!(rx.recv_timeout(Duration::from_millis(10)).is_err());
    }

    #[test]
    fn disconnected_client_is_pruned() {
        let clients = ClientHub::new(256);
        drop(clients.register());
        assert_eq!(clients.broadcast_str("x"), 0);
        assert_eq!(clients.client_count(), 0);
    }

    #[test]
    fn delivers_to_all_in_order() {
        let clients = ClientHub::new(256);
        let a = clients.register();
        let b = clients.register();
        clients.broadcast_str("one");
        clients.broadcast_str("two");
        for rx in [&a, &b] {
            assert_eq!(&*rx.try_recv().unwrap(), "one");
            assert_eq!(&*rx.try_recv().unwrap(), "two");
        }
    }
}
