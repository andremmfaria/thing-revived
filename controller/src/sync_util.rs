//! Small shared locking helper.

use std::sync::{Mutex, MutexGuard};

/// Locks `m`, recovering the guard if a previous holder panicked.
///
/// Every mutex shared across this daemon's threads guards plain data
/// (client list, settings, Wi-Fi caches) that stays structurally valid after
/// a panic, so one panicking thread must not poison the lock and make every
/// other thread (including the button/knob loops) panic on its next
/// `.lock().unwrap()`.
pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Held by tests that write an executable script and then run it, or that
/// spawn processes: a concurrent `fork` in another test thread can inherit the
/// script's still-open write fd and make `exec` fail with ETXTBSY.
#[cfg(test)]
pub static SPAWN_GUARD: Mutex<()> = Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn lock_recovers_after_a_holder_panicked() {
        let m = Arc::new(Mutex::new(5));
        let m2 = m.clone();
        let _ = std::thread::spawn(move || {
            let _g = m2.lock().unwrap();
            panic!("poison it");
        })
        .join();
        assert!(m.is_poisoned());
        assert_eq!(*lock(&m), 5);
    }
}
