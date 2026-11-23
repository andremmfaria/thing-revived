//! Account management logic (pure, no I/O). Phase 1 holds the first-boot
//! password generator and the first-boot state machine; later phases add
//! names, guard rails and sessions next to them.

pub mod password;
pub mod state;

// Phase 2a and 2b: pure account domain (names, snapshot, guard rails, rate
// limits, sessions, error codes, group policy, authorized-key parser).
pub mod events;
pub mod groups;
pub mod guard;
pub mod keys;
pub mod model;
pub mod names;
pub mod ratelimit;
pub mod session;
