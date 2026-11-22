//! Client fanout with bounded per-client queues.
//!
//! The PTY reader must never block on, or accumulate unbounded memory for, a
//! slow client. Each client gets a fixed-capacity queue; if it is full the
//! client is dropped (its sender is removed, which makes the writer thread
//! see a disconnect and close the socket) instead of stalling the reader.

use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};

/// Messages queued per client before it is considered stalled. Output
/// messages carry at most one 4096-byte read, so this bounds a stalled
/// client to roughly 1 MiB of text (a few MiB worst case with JSON escaping).
pub const CLIENT_QUEUE_CAP: usize = 256;

/// Shared message: one serialization, cheap per-client clone under the lock.
pub type Msg = Arc<str>;
pub type Clients = Arc<Mutex<Vec<SyncSender<Msg>>>>;

pub fn new_clients() -> Clients {
    Arc::new(Mutex::new(Vec::new()))
}

/// Registers a client and returns its receiving end.
pub fn register(clients: &Clients) -> Receiver<Msg> {
    register_with_cap(clients, CLIENT_QUEUE_CAP)
}

pub fn register_with_cap(clients: &Clients, cap: usize) -> Receiver<Msg> {
    let (tx, rx) = sync_channel::<Msg>(cap);
    clients.lock().unwrap().push(tx);
    rx
}

/// Queues `json` for every client without blocking. Clients whose queue is
/// full or whose receiver is gone are removed. Returns how many were dropped
/// for being full. Only try_send (no I/O, no waiting) runs under the lock.
pub fn broadcast_str(clients: &Clients, json: &str) -> usize {
    let msg: Msg = Arc::from(json);
    let mut dropped_full = 0;
    let mut guard = clients.lock().unwrap();
    guard.retain(|tx| match tx.try_send(msg.clone()) {
        Ok(()) => true,
        Err(TrySendError::Full(_)) => {
            dropped_full += 1;
            false
        }
        Err(TrySendError::Disconnected(_)) => false,
    });
    dropped_full
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn stalled_client_is_dropped_without_blocking() {
        let clients = new_clients();
        let _stalled = register_with_cap(&clients, 4); // never read
        let healthy = register_with_cap(&clients, 4);
        let start = Instant::now();
        let mut total_dropped = 0;
        for i in 0..50 {
            total_dropped += broadcast_str(&clients, &format!("m{i}"));
            // The healthy client keeps draining.
            while healthy.try_recv().is_ok() {}
        }
        assert!(start.elapsed() < Duration::from_secs(1));
        assert_eq!(total_dropped, 1);
        assert_eq!(clients.lock().unwrap().len(), 1);
    }

    #[test]
    fn queue_memory_is_bounded_by_capacity() {
        let clients = new_clients();
        let rx = register_with_cap(&clients, 8);
        for i in 0..1000 {
            broadcast_str(&clients, &format!("{i}"));
        }
        // Only the first 8 were queued; the client was then dropped.
        assert_eq!(rx.try_iter().count(), 8);
        assert!(clients.lock().unwrap().is_empty());
        // Sender dropped, so a writer thread sees Disconnected after draining.
        assert!(rx.recv_timeout(Duration::from_millis(10)).is_err());
    }

    #[test]
    fn disconnected_client_is_pruned() {
        let clients = new_clients();
        drop(register(&clients));
        assert_eq!(broadcast_str(&clients, "x"), 0);
        assert!(clients.lock().unwrap().is_empty());
    }

    #[test]
    fn delivers_to_all_in_order() {
        let clients = new_clients();
        let a = register(&clients);
        let b = register(&clients);
        broadcast_str(&clients, "one");
        broadcast_str(&clients, "two");
        for rx in [&a, &b] {
            assert_eq!(&*rx.try_recv().unwrap(), "one");
            assert_eq!(&*rx.try_recv().unwrap(), "two");
        }
    }
}
