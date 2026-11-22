//! Account management logic (pure, no I/O). Phase 1 holds the first-boot
//! password generator and the first-boot state machine; later phases add
//! names, guard rails and sessions next to them.

pub mod password;
pub mod state;
