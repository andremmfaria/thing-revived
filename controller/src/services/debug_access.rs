//! `DebugAccessService`: the `debug_get`/`debug_set` handlers (plan section
//! 5, delta per this task's brief). Mirrors `usb_role.rs`'s shape: one
//! mutation mutex, fixed input validated against a whitelist, status
//! always re-read fresh, never trusted from the write's own exit code.
//!
//! Order of checks on `debug_set`, exactly, before any systemctl call:
//! admin session unlocked, then the per-connection rate limit, then the
//! surface whitelist, then `confirm`, then (adb enable only) the live USB
//! role, then the mutation mutex. Only past all of that does a systemctl
//! call happen; the status is always re-read and broadcast afterwards,
//! success or failure, so the badge never shows a guess.

use crate::app::hub::ConnId;
use crate::domain::debug_access::{guard_adb_enable, toggle_error_code, validate_surface, DebugAccessErrorCode, InFlight, Surface};
use crate::ports::{AdminSessionGate, DebugAccessCtl, EventSink, UsbRoleCtl};
use crate::protocol::BridgeEvent;
use crate::sync_util::lock;
use log::{info, warn};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub struct DebugAccessService {
    ctl: Arc<dyn DebugAccessCtl>,
    usb_role: Arc<dyn UsbRoleCtl>,
    gate: Arc<dyn AdminSessionGate>,
    sink: Arc<dyn EventSink>,
    /// One toggle at a time; a second concurrent `debug_set` is refused
    /// with `busy` rather than queued.
    mutate: Mutex<()>,
    /// Shared with `UsbRoleService::reboot` (plan section 12).
    in_flight: Arc<InFlight>,
    /// Which connection most recently enabled each surface at runtime, so
    /// the auto-disable hook reverts only what that session turned on.
    enabled_by: Mutex<HashMap<Surface, u64>>,
    /// Per-connection `debug_set` timestamps for the rate limiter.
    rate: Mutex<HashMap<u64, Vec<Instant>>>,
    rate_max: usize,
    rate_window: Duration,
}

impl DebugAccessService {
    pub fn new(
        ctl: Arc<dyn DebugAccessCtl>,
        usb_role: Arc<dyn UsbRoleCtl>,
        gate: Arc<dyn AdminSessionGate>,
        sink: Arc<dyn EventSink>,
        in_flight: Arc<InFlight>,
    ) -> DebugAccessService {
        DebugAccessService::with_rate_limit(ctl, usb_role, gate, sink, in_flight, 6, Duration::from_secs(60))
    }

    /// Exposed separately so tests can use a window short enough to wait
    /// out in real time, without changing the production default (6 per
    /// 60s).
    pub fn with_rate_limit(
        ctl: Arc<dyn DebugAccessCtl>,
        usb_role: Arc<dyn UsbRoleCtl>,
        gate: Arc<dyn AdminSessionGate>,
        sink: Arc<dyn EventSink>,
        in_flight: Arc<InFlight>,
        rate_max: usize,
        rate_window: Duration,
    ) -> DebugAccessService {
        DebugAccessService {
            ctl,
            usb_role,
            gate,
            sink,
            mutate: Mutex::new(()),
            in_flight,
            enabled_by: Mutex::new(HashMap::new()),
            rate: Mutex::new(HashMap::new()),
            rate_max,
            rate_window,
        }
    }

    /// Registers this service's auto-disable hook on the admin session
    /// gate. Called once at startup, after both are constructed.
    pub fn register_auto_disable(self: &Arc<Self>) {
        let this = self.clone();
        self.gate.on_session_end(Box::new(move |conn| this.on_session_end(conn)));
    }

    // -----------------------------------------------------------------
    // Status
    // -----------------------------------------------------------------

    fn live(&self, surface: Surface) -> bool {
        self.ctl.status(surface).unwrap_or_else(|e| {
            warn!("debug access: status({}) failed, reporting disabled: {e}", surface.as_str());
            false
        })
    }

    fn build_default(&self, surface: Surface) -> bool {
        match surface {
            // The build side only ever wrote the adb marker
            // (`/etc/usb-debugging-enabled`); the existing USB-role port
            // already reads it fresh on every call, so this reuses it
            // instead of adding a second file read.
            Surface::Adb => self.usb_role.adb_forced(),
            Surface::Uart => self.ctl.build_default(Surface::Uart).unwrap_or_else(|e| {
                warn!("debug access: build_default(uart) failed, reporting not-default-on: {e}");
                false
            }),
        }
    }

    fn adb_availability(&self) -> (bool, Option<String>) {
        let role = self.usb_role.status().ok().and_then(|s| s.role);
        match guard_adb_enable(role) {
            Ok(()) => (true, None),
            Err(code) => (false, Some(code.as_str().to_string())),
        }
    }

    /// Re-queries both surfaces fresh and broadcasts the result; never
    /// cached (plan section 5.5).
    fn report(&self) {
        let adb = self.live(Surface::Adb);
        let uart = self.live(Surface::Uart);
        let adb_build_default = self.build_default(Surface::Adb);
        let uart_build_default = self.build_default(Surface::Uart);
        let (adb_available, blocked) = self.adb_availability();
        self.sink.emit(BridgeEvent::DebugState { adb, uart, adb_build_default, uart_build_default, adb_available, blocked });
    }

    /// `debug_get`.
    pub fn get(&self) {
        self.report();
    }

    fn fail(&self, context: &str, code: DebugAccessErrorCode, message: impl Into<String>) {
        self.sink.emit(BridgeEvent::DebugError { context: context.into(), code: code.as_str().into(), message: message.into() });
    }

    // -----------------------------------------------------------------
    // Mutation
    // -----------------------------------------------------------------

    fn rate_limited(&self, conn: ConnId) -> bool {
        let now = Instant::now();
        let mut guard = lock(&self.rate);
        let hits = guard.entry(conn.0).or_default();
        hits.retain(|t| now.duration_since(*t) < self.rate_window);
        if hits.len() >= self.rate_max {
            return true;
        }
        hits.push(now);
        false
    }

    /// `debug_set`. `surface`/`confirm` are untrusted client strings;
    /// `enabled` is the requested target state.
    pub fn set(&self, conn: ConnId, surface: &str, enabled: bool, confirm: &str) {
        const CTX: &str = "debug_set";

        // (1) Admin session, before anything else, including parsing.
        if self.gate.require(conn).is_err() {
            return self.fail(CTX, DebugAccessErrorCode::NotUnlocked, "Sign in as an admin to change debug access");
        }
        // Rate limit right after the gate: an unlocked session is still
        // capped, matching `AccountsService`'s own mutation limiter shape.
        if self.rate_limited(conn) {
            return self.fail(CTX, DebugAccessErrorCode::RateLimited, "Too many attempts. Wait and try again");
        }
        // (2) Surface whitelist.
        let surface = match validate_surface(surface) {
            Ok(s) => s,
            Err(e) => return self.fail(CTX, DebugAccessErrorCode::InvalidSurface, e.message),
        };
        // (3) Confirm must echo the surface exactly; no typed confirm from
        // the user, the hold gesture is the proof of intent (plan section
        // 12), but the server still requires the field to equal the
        // surface string.
        if confirm != surface.as_str() {
            return self.fail(CTX, DebugAccessErrorCode::ConfirmRequired, "Hold the toggle to confirm");
        }
        // (4) Enabling adb needs the live USB role to be Device.
        if surface == Surface::Adb && enabled {
            let role = self.usb_role.status().ok().and_then(|s| s.role);
            if let Err(code) = guard_adb_enable(role) {
                return self.fail(CTX, code, "Switch the USB port to device mode first");
            }
        }
        // (5) One toggle at a time.
        let Ok(_guard) = self.mutate.try_lock() else {
            return self.fail(CTX, DebugAccessErrorCode::Busy, "Another debug access change is already in progress");
        };
        self.in_flight.set(true);
        let _reset_in_flight = InFlightGuard(&self.in_flight);

        // (6) The actual systemctl call.
        let outcome = if enabled { self.ctl.enable(surface) } else { self.ctl.disable(surface) };
        match outcome {
            Ok(()) => {
                if enabled {
                    lock(&self.enabled_by).insert(surface, conn.0);
                } else {
                    lock(&self.enabled_by).remove(&surface);
                }
                info!("debug access: {} {} (conn {})", surface.as_str(), if enabled { "enabled" } else { "disabled" }, conn.0);
            }
            Err(e) => {
                warn!("{CTX}: {e}");
                if enabled {
                    // Never leave a half-applied enable reachable: best
                    // effort, its own failure only gets a log line.
                    match self.ctl.disable(surface) {
                        Ok(()) => {
                            lock(&self.enabled_by).remove(&surface);
                        }
                        Err(e2) => warn!("{CTX}: best-effort disable after a failed enable also failed: {e2}"),
                    }
                }
                self.fail(CTX, toggle_error_code(&e), e.message.clone());
            }
        }
        // (7) Always re-read and broadcast the truth, success or failure.
        self.report();
    }

    // -----------------------------------------------------------------
    // Auto-disable on admin session end (owner decision, plan section 12)
    // -----------------------------------------------------------------

    /// Called through `AdminSessionGate::on_session_end` for every
    /// connection whose admin session just ended, for any reason. Reverts
    /// only the surfaces this exact connection enabled at runtime, and
    /// only down to their build default (a surface already at its build
    /// default, on or off, is left alone).
    fn on_session_end(&self, conn: ConnId) {
        let mut changed = false;
        for surface in [Surface::Adb, Surface::Uart] {
            let owner = lock(&self.enabled_by).get(&surface).copied();
            if owner != Some(conn.0) {
                continue;
            }
            lock(&self.enabled_by).remove(&surface);
            let live = self.live(surface);
            let build_default = self.build_default(surface);
            if live && live != build_default {
                info!("debug access: auto-disabling {} (admin session on conn {} ended)", surface.as_str(), conn.0);
                if let Err(e) = self.ctl.disable(surface) {
                    warn!("debug access: auto-disable of {} failed: {e}", surface.as_str());
                }
                changed = true;
            }
        }
        if changed {
            self.report();
        }
    }
}

/// Clears the shared in-flight flag when a `debug_set` call ends, by any
/// path (success, failure, or an early return before this point is
/// reached simply never constructs the guard).
struct InFlightGuard<'a>(&'a InFlight);

impl Drop for InFlightGuard<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::hub::RecordingSink;
    use crate::domain::usb_role::{UsbRole, UsbRoleStatus};
    use crate::utils::types::AppError;
    use std::sync::Mutex as StdMutex;

    struct FakeCtl {
        status: StdMutex<HashMap<&'static str, bool>>,
        build_default: StdMutex<HashMap<&'static str, bool>>,
        enable_err: StdMutex<Option<AppError>>,
        disable_err: StdMutex<Option<AppError>>,
        calls: StdMutex<Vec<String>>,
    }

    impl FakeCtl {
        fn new() -> Arc<FakeCtl> {
            Arc::new(FakeCtl {
                status: StdMutex::new([("adb", false), ("uart", false)].into_iter().collect()),
                build_default: StdMutex::new([("adb", false), ("uart", false)].into_iter().collect()),
                enable_err: StdMutex::new(None),
                disable_err: StdMutex::new(None),
                calls: StdMutex::new(vec![]),
            })
        }

        fn set_status(&self, surface: Surface, value: bool) {
            self.status.lock().unwrap().insert(surface.as_str(), value);
        }
    }

    impl DebugAccessCtl for FakeCtl {
        fn status(&self, surface: Surface) -> Result<bool, AppError> {
            Ok(*self.status.lock().unwrap().get(surface.as_str()).unwrap_or(&false))
        }
        fn enable(&self, surface: Surface) -> Result<(), AppError> {
            self.calls.lock().unwrap().push(format!("enable {}", surface.as_str()));
            if let Some(e) = self.enable_err.lock().unwrap().clone() {
                return Err(e);
            }
            self.set_status(surface, true);
            Ok(())
        }
        fn disable(&self, surface: Surface) -> Result<(), AppError> {
            self.calls.lock().unwrap().push(format!("disable {}", surface.as_str()));
            if let Some(e) = self.disable_err.lock().unwrap().clone() {
                return Err(e);
            }
            self.set_status(surface, false);
            Ok(())
        }
        fn build_default(&self, surface: Surface) -> Result<bool, AppError> {
            Ok(*self.build_default.lock().unwrap().get(surface.as_str()).unwrap_or(&false))
        }
    }

    struct FakeUsb {
        role: StdMutex<Option<UsbRole>>,
        forced: bool,
    }

    impl FakeUsb {
        fn new(role: Option<UsbRole>, forced: bool) -> Arc<FakeUsb> {
            Arc::new(FakeUsb { role: StdMutex::new(role), forced })
        }
    }

    impl UsbRoleCtl for FakeUsb {
        fn status(&self) -> Result<UsbRoleStatus, AppError> {
            Ok(UsbRoleStatus { role: *self.role.lock().unwrap(), boot_role: None })
        }
        fn persist(&self, _role: UsbRole) -> Result<(), AppError> {
            Ok(())
        }
        fn reboot(&self) -> Result<(), AppError> {
            Ok(())
        }
        fn adb_forced(&self) -> bool {
            self.forced
        }
    }

    /// Records `require` calls so a test can assert the gate is consulted
    /// before anything else; `locked()` makes every connection unlocked.
    type SessionEndHooks = StdMutex<Vec<Box<dyn Fn(ConnId) + Send + Sync>>>;

    struct FakeGate {
        locked: bool,
        calls: StdMutex<Vec<ConnId>>,
        hooks: SessionEndHooks,
    }

    impl FakeGate {
        fn unlocked() -> Arc<FakeGate> {
            Arc::new(FakeGate { locked: false, calls: StdMutex::new(vec![]), hooks: StdMutex::new(vec![]) })
        }
        fn locked() -> Arc<FakeGate> {
            Arc::new(FakeGate { locked: true, calls: StdMutex::new(vec![]), hooks: StdMutex::new(vec![]) })
        }
        fn end_session(&self, conn: ConnId) {
            for hook in self.hooks.lock().unwrap().iter() {
                hook(conn);
            }
        }
    }

    impl AdminSessionGate for FakeGate {
        fn require(&self, conn: ConnId) -> Result<String, AppError> {
            self.calls.lock().unwrap().push(conn);
            if self.locked {
                Err(AppError::invalid("not_unlocked"))
            } else {
                Ok("kiosk".into())
            }
        }
        fn on_session_end(&self, cb: Box<dyn Fn(ConnId) + Send + Sync>) {
            self.hooks.lock().unwrap().push(cb);
        }
    }

    const CONN: ConnId = ConnId(1);
    const OTHER: ConnId = ConnId(2);

    type Rig = (Arc<DebugAccessService>, Arc<RecordingSink>, Arc<FakeCtl>, Arc<FakeUsb>, Arc<FakeGate>);

    fn rig() -> Rig {
        rig_with(FakeUsb::new(Some(UsbRole::Device), false), FakeGate::unlocked())
    }

    fn rig_with(usb: Arc<FakeUsb>, gate: Arc<FakeGate>) -> Rig {
        let sink = Arc::new(RecordingSink::new());
        let ctl = FakeCtl::new();
        let dyn_sink: Arc<dyn EventSink> = sink.clone();
        let dyn_gate: Arc<dyn AdminSessionGate> = gate.clone();
        let dyn_usb: Arc<dyn UsbRoleCtl> = usb.clone();
        let svc = Arc::new(DebugAccessService::new(ctl.clone(), dyn_usb, dyn_gate, dyn_sink, Arc::new(InFlight::new())));
        svc.register_auto_disable();
        (svc, sink, ctl, usb, gate)
    }

    fn only_state(sink: &RecordingSink) -> BridgeEvent {
        let mut got = sink.take();
        assert_eq!(got.len(), 1, "{got:?}");
        got.remove(0)
    }

    #[test]
    fn get_reports_live_status_build_default_and_availability() {
        let (svc, sink, ctl, _usb, _gate) = rig();
        ctl.set_status(Surface::Adb, true);
        ctl.build_default.lock().unwrap().insert("uart", true);
        svc.get();
        assert!(matches!(
            only_state(&sink),
            BridgeEvent::DebugState { adb: true, uart: false, adb_build_default: false, uart_build_default: true, adb_available: true, blocked: None }
        ));
    }

    #[test]
    fn get_reports_host_role_as_blocked_and_unavailable() {
        let (svc, sink, _ctl, usb, _gate) = rig();
        *usb.role.lock().unwrap() = Some(UsbRole::Host);
        svc.get();
        assert!(matches!(
            only_state(&sink),
            BridgeEvent::DebugState { adb_available: false, blocked: Some(ref c), .. } if c == "usb_role_host"
        ));
    }

    #[test]
    fn set_checks_the_gate_before_touching_the_adapter_and_before_parsing() {
        let (svc, sink, ctl, _usb, gate) = rig_with(FakeUsb::new(Some(UsbRole::Device), false), FakeGate::locked());
        svc.set(CONN, "not a surface at all", true, "also bogus");
        assert_eq!(gate.calls.lock().unwrap().len(), 1, "the gate must be consulted exactly once, first");
        assert!(ctl.calls.lock().unwrap().is_empty(), "a locked session must never reach the adapter");
        assert!(matches!(&sink.take()[..], [BridgeEvent::DebugError { context, code, .. }] if context == "debug_set" && code == "not_unlocked"));
    }

    #[test]
    fn set_rejects_an_invalid_surface_without_touching_the_adapter() {
        let (svc, sink, ctl, ..) = rig();
        svc.set(CONN, "both", true, "both");
        assert!(ctl.calls.lock().unwrap().is_empty());
        assert!(matches!(&sink.take()[..], [BridgeEvent::DebugError { code, .. }] if code == "invalid_surface"));
    }

    #[test]
    fn set_requires_confirm_to_equal_the_surface() {
        let (svc, sink, ctl, ..) = rig();
        svc.set(CONN, "adb", true, "uart");
        assert!(ctl.calls.lock().unwrap().is_empty());
        assert!(matches!(&sink.take()[..], [BridgeEvent::DebugError { code, .. }] if code == "confirm_required"));
    }

    #[test]
    fn set_refuses_adb_enable_while_the_usb_role_is_host() {
        let (svc, sink, ctl, _usb, _gate) = rig_with(FakeUsb::new(Some(UsbRole::Host), false), FakeGate::unlocked());
        svc.set(CONN, "adb", true, "adb");
        assert!(ctl.calls.lock().unwrap().is_empty());
        assert!(matches!(&sink.take()[..], [BridgeEvent::DebugError { code, .. }] if code == "usb_role_host"));
    }

    #[test]
    fn set_allows_adb_disable_even_while_the_usb_role_is_host() {
        let (svc, sink, ctl, _usb, _gate) = rig_with(FakeUsb::new(Some(UsbRole::Host), false), FakeGate::unlocked());
        ctl.set_status(Surface::Adb, true);
        svc.set(CONN, "adb", false, "adb");
        assert_eq!(ctl.calls.lock().unwrap().clone(), vec!["disable adb"]);
        assert!(matches!(&sink.take()[..], [BridgeEvent::DebugState { adb: false, .. }]));
    }

    #[test]
    fn set_enables_then_disables_and_always_rereads_status() {
        let (svc, sink, ctl, ..) = rig();
        svc.set(CONN, "adb", true, "adb");
        assert_eq!(ctl.calls.lock().unwrap().clone(), vec!["enable adb"]);
        assert!(matches!(&sink.take()[..], [BridgeEvent::DebugState { adb: true, .. }]));
        svc.set(CONN, "adb", false, "adb");
        assert_eq!(ctl.calls.lock().unwrap().clone(), vec!["enable adb", "disable adb"]);
        assert!(matches!(&sink.take()[..], [BridgeEvent::DebugState { adb: false, .. }]));
    }

    #[test]
    fn a_second_concurrent_set_is_busy_and_never_touches_the_adapter() {
        let (svc, sink, ctl, ..) = rig();
        let _held = svc.mutate.try_lock().unwrap();
        svc.set(CONN, "adb", true, "adb");
        assert!(ctl.calls.lock().unwrap().is_empty());
        assert!(matches!(&sink.take()[..], [BridgeEvent::DebugError { code, .. }] if code == "busy"));
    }

    #[test]
    fn a_failing_enable_is_toggle_failed_and_leaves_the_true_status_broadcast() {
        let (svc, sink, ctl, ..) = rig();
        *ctl.enable_err.lock().unwrap() = Some(AppError::failed("systemctl start x failed: boom"));
        svc.set(CONN, "adb", true, "adb");
        let got = sink.take();
        assert_eq!(got.len(), 2, "{got:?}");
        assert!(matches!(&got[0], BridgeEvent::DebugError { code, .. } if code == "toggle_failed"));
        assert!(matches!(&got[1], BridgeEvent::DebugState { adb: false, .. }), "the adapter never actually enabled it");
        assert_eq!(ctl.calls.lock().unwrap().clone(), vec!["enable adb", "disable adb"], "a best-effort disable follows a failed enable");
    }

    #[test]
    fn rate_limit_kicks_in_after_the_configured_count() {
        let sink = Arc::new(RecordingSink::new());
        let ctl = FakeCtl::new();
        let usb = FakeUsb::new(Some(UsbRole::Device), false);
        let gate = FakeGate::unlocked();
        let dyn_sink: Arc<dyn EventSink> = sink.clone();
        let dyn_gate: Arc<dyn AdminSessionGate> = gate.clone();
        let dyn_usb: Arc<dyn UsbRoleCtl> = usb.clone();
        let svc = DebugAccessService::with_rate_limit(ctl.clone(), dyn_usb, dyn_gate, dyn_sink, Arc::new(InFlight::new()), 2, Duration::from_millis(200));
        svc.set(CONN, "adb", true, "adb");
        sink.take();
        svc.set(CONN, "adb", false, "adb");
        sink.take();
        svc.set(CONN, "adb", true, "adb");
        assert!(matches!(&sink.take()[..], [BridgeEvent::DebugError { code, .. }] if code == "rate_limited"));
        std::thread::sleep(Duration::from_millis(220));
        svc.set(CONN, "adb", false, "adb");
        assert!(matches!(&sink.take()[..], [BridgeEvent::DebugState { .. }]), "the window rolled over");
    }

    #[test]
    fn auto_disable_reverts_only_what_this_connection_enabled_above_build_default() {
        let (svc, sink, ctl, _usb, gate) = rig();
        svc.set(CONN, "adb", true, "adb");
        sink.take();
        svc.set(OTHER, "uart", true, "uart");
        sink.take();

        // A different session (not CONN, not OTHER) ending touches nothing.
        gate.end_session(ConnId(99));
        assert!(sink.take().is_empty(), "an unrelated session ending must change nothing");
        assert!(ctl.status(Surface::Adb).unwrap() && ctl.status(Surface::Uart).unwrap());

        // CONN's session ends: only adb (which CONN enabled) reverts.
        gate.end_session(CONN);
        assert!(!ctl.status(Surface::Adb).unwrap(), "adb reverts to its build default (off)");
        assert!(ctl.status(Surface::Uart).unwrap(), "uart was OTHER's, untouched by CONN's session ending");
        assert!(matches!(&sink.take()[..], [BridgeEvent::DebugState { adb: false, uart: true, .. }]));

        // OTHER's session ends next: uart now reverts too.
        gate.end_session(OTHER);
        assert!(!ctl.status(Surface::Uart).unwrap());
        assert!(matches!(&sink.take()[..], [BridgeEvent::DebugState { adb: false, uart: false, .. }]));

        // Ending CONN's session again is a no-op: the attribution was
        // already cleared, and nothing is left to revert.
        gate.end_session(CONN);
        assert!(sink.take().is_empty());
    }

    #[test]
    fn auto_disable_leaves_a_surface_alone_once_it_matches_its_build_default() {
        let (svc, sink, ctl, _usb, gate) = rig();
        // uart's build default is already on; enabling it at runtime is a
        // no-op relative to the default, so a session end must not touch it.
        ctl.build_default.lock().unwrap().insert("uart", true);
        ctl.set_status(Surface::Uart, true);
        svc.set(CONN, "uart", true, "uart");
        sink.take();

        gate.end_session(CONN);
        assert!(sink.take().is_empty(), "uart already matches its build default, so nothing should change");
        assert!(ctl.status(Surface::Uart).unwrap(), "uart stays on: it was already the build default");
    }
}
