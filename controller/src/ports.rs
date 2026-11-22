//! Ports: the traits the application layer depends on. Adapters implement
//! them; `main` (via `run`) is the only place that picks concrete types.

use crate::protocol::BridgeEvent;

/// Where services send events for clients. Implemented by `app::hub::EventHub`
/// and, in tests, by `app::hub::RecordingSink`.
pub trait EventSink: Send + Sync {
    fn emit(&self, e: BridgeEvent);
}
