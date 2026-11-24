//! USB role commands: report the running and persisted role, persist a new
//! boot role, reboot. The role is only ever persisted, never switched live
//! (`to-host` would drop the link the request may have arrived over). A
//! client string is validated against the role whitelist before the port
//! sees it.

use crate::domain::debug_access::InFlight;
use crate::domain::usb_role::{validate_role, UsbRole, UsbRoleStatus};
use crate::ports::{EventSink, UsbRoleCtl};
use crate::protocol::BridgeEvent;
use crate::utils::types::{AppError, ErrorKind};
use log::{error, info};
use std::sync::Arc;

pub struct UsbRoleService {
    ctl: Arc<dyn UsbRoleCtl>,
    sink: Arc<dyn EventSink>,
    /// Shared with `DebugAccessService` (owner decision, plan section 12):
    /// true while a debug-access toggle holds its mutation mutex, so a
    /// reboot cannot race a half-applied adb/uart switch.
    debug_toggle_in_flight: Arc<InFlight>,
}

fn code(e: &AppError) -> &'static str {
    match e.kind {
        ErrorKind::Invalid => "invalid_role",
        ErrorKind::Unavailable => "unavailable",
        ErrorKind::Failed => "failed",
    }
}

impl UsbRoleService {
    pub fn new(ctl: Arc<dyn UsbRoleCtl>, sink: Arc<dyn EventSink>, debug_toggle_in_flight: Arc<InFlight>) -> UsbRoleService {
        UsbRoleService { ctl, sink, debug_toggle_in_flight }
    }

    fn fail(&self, context: &str, e: &AppError) {
        error!("{context}: {e}");
        self.sink.emit(BridgeEvent::UsbRoleError { context: context.into(), code: code(e).into(), message: e.message.clone() });
    }

    fn report(&self, status: UsbRoleStatus) {
        self.sink.emit(BridgeEvent::UsbRole {
            role: status.role.map(|r| r.as_str().to_string()),
            boot_role: status.boot_role.map(|r| r.as_str().to_string()),
            adb_forced: self.ctl.adb_forced(),
        });
    }

    /// `usb_role_get`.
    pub fn get(&self) {
        match self.ctl.status() {
            Ok(status) => self.report(status),
            Err(e) => self.fail("usb_role_get", &e),
        }
    }

    /// `usb_role_set`: persist only, applies from the next boot.
    pub fn set(&self, role: &str) {
        let role: UsbRole = match validate_role(role) {
            Ok(r) => r,
            Err(e) => return self.fail("usb_role_set", &e),
        };
        if let Err(e) = self.ctl.persist(role) {
            return self.fail("usb_role_set", &e);
        }
        info!("usb role: boot role persisted as {}", role.as_str());
        // Re-read instead of echoing the request: the event shows what the tool recorded.
        match self.ctl.status() {
            Ok(status) => self.report(status),
            Err(e) => self.fail("usb_role_set", &e),
        }
    }

    /// `usb_role_reboot`. Refused while a debug-access toggle is in
    /// flight (owner decision, plan section 12): rebooting mid-toggle
    /// would race the systemctl calls that bring a surface up or down.
    /// Reported through the existing `usb_role_error` vocabulary, since
    /// this plan added no new code to it.
    pub fn reboot(&self) {
        if self.debug_toggle_in_flight.get() {
            return self.fail(
                "usb_role_reboot",
                &AppError::failed("a debug access toggle is in progress; try the reboot again shortly"),
            );
        }
        match self.ctl.reboot() {
            Ok(()) => {
                info!("usb role: reboot requested");
                self.sink.emit(BridgeEvent::UsbRoleRebooting {});
            }
            Err(e) => self.fail("usb_role_reboot", &e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::hub::RecordingSink;
    use std::sync::Mutex;

    pub struct FakeCtl {
        pub status: Mutex<Result<UsbRoleStatus, AppError>>,
        pub persisted: Mutex<Vec<UsbRole>>,
        pub reboots: Mutex<u32>,
        pub persist_err: Mutex<Option<AppError>>,
        pub adb: bool,
    }

    impl FakeCtl {
        fn new(adb: bool) -> Arc<FakeCtl> {
            Arc::new(FakeCtl {
                status: Mutex::new(Ok(UsbRoleStatus { role: Some(UsbRole::Device), boot_role: None })),
                persisted: Mutex::new(vec![]),
                reboots: Mutex::new(0),
                persist_err: Mutex::new(None),
                adb,
            })
        }
    }

    impl UsbRoleCtl for FakeCtl {
        fn status(&self) -> Result<UsbRoleStatus, AppError> {
            self.status.lock().unwrap().clone()
        }
        fn persist(&self, role: UsbRole) -> Result<(), AppError> {
            if let Some(e) = self.persist_err.lock().unwrap().clone() {
                return Err(e);
            }
            self.persisted.lock().unwrap().push(role);
            let mut s = self.status.lock().unwrap();
            if let Ok(st) = s.as_mut() {
                st.boot_role = Some(role);
            }
            Ok(())
        }
        fn reboot(&self) -> Result<(), AppError> {
            *self.reboots.lock().unwrap() += 1;
            Ok(())
        }
        fn adb_forced(&self) -> bool {
            self.adb
        }
    }

    fn rig(adb: bool) -> (UsbRoleService, Arc<RecordingSink>, Arc<FakeCtl>) {
        let sink = Arc::new(RecordingSink::new());
        let ctl = FakeCtl::new(adb);
        (UsbRoleService::new(ctl.clone(), sink.clone(), Arc::new(InFlight::new())), sink, ctl)
    }

    #[test]
    fn get_reports_role_boot_role_and_the_adb_flag() {
        let (svc, sink, _) = rig(true);
        svc.get();
        assert!(matches!(&sink.take()[..], [BridgeEvent::UsbRole { role: Some(r), boot_role: None, adb_forced: true }] if r == "device"));
    }

    #[test]
    fn get_failure_is_an_error_event_with_a_code() {
        let (svc, sink, ctl) = rig(false);
        *ctl.status.lock().unwrap() = Err(AppError::failed("spawn superbird-usb-role: nope"));
        svc.get();
        assert!(matches!(&sink.take()[..], [BridgeEvent::UsbRoleError { context, code, .. }] if context == "usb_role_get" && code == "failed"));
    }

    #[test]
    fn set_persists_then_rereads_and_broadcasts() {
        let (svc, sink, ctl) = rig(false);
        svc.set("host");
        assert_eq!(ctl.persisted.lock().unwrap().clone(), vec![UsbRole::Host]);
        assert!(matches!(&sink.take()[..], [BridgeEvent::UsbRole { role: Some(r), boot_role: Some(b), adb_forced: false }] if r == "device" && b == "host"));
    }

    #[test]
    fn set_rejects_anything_outside_the_whitelist_without_touching_the_port() {
        let (svc, sink, ctl) = rig(false);
        for bad in ["", "Host", "host; reboot", "clear", "--help", "to-host"] {
            svc.set(bad);
            assert!(matches!(&sink.take()[..], [BridgeEvent::UsbRoleError { context, code, .. }] if context == "usb_role_set" && code == "invalid_role"), "{bad:?}");
        }
        assert!(ctl.persisted.lock().unwrap().is_empty());
    }

    #[test]
    fn a_failing_persist_reports_failed_and_sends_no_role_event() {
        let (svc, sink, ctl) = rig(false);
        *ctl.persist_err.lock().unwrap() = Some(AppError::failed("superbird-usb-role persist failed: read-only"));
        svc.set("host");
        assert!(matches!(&sink.take()[..], [BridgeEvent::UsbRoleError { code, message, .. }] if code == "failed" && message.contains("read-only")));
    }

    #[test]
    fn unavailable_tool_maps_to_the_unavailable_code() {
        let (svc, sink, ctl) = rig(false);
        *ctl.status.lock().unwrap() = Err(AppError::new(ErrorKind::Unavailable, "no tool"));
        svc.get();
        assert!(matches!(&sink.take()[..], [BridgeEvent::UsbRoleError { code, .. }] if code == "unavailable"));
    }

    #[test]
    fn reboot_runs_the_port_and_announces() {
        let (svc, sink, ctl) = rig(false);
        svc.reboot();
        assert_eq!(*ctl.reboots.lock().unwrap(), 1);
        assert!(matches!(&sink.take()[..], [BridgeEvent::UsbRoleRebooting {}]));
    }

    #[test]
    fn reboot_is_refused_while_a_debug_access_toggle_is_in_flight() {
        let sink = Arc::new(RecordingSink::new());
        let ctl = FakeCtl::new(false);
        let in_flight = Arc::new(InFlight::new());
        in_flight.set(true);
        let svc = UsbRoleService::new(ctl.clone(), sink.clone(), in_flight.clone());
        svc.reboot();
        assert_eq!(*ctl.reboots.lock().unwrap(), 0, "the port must not be called");
        assert!(matches!(&sink.take()[..], [BridgeEvent::UsbRoleError { context, code, .. }] if context == "usb_role_reboot" && code == "failed"));
        in_flight.set(false);
        svc.reboot();
        assert_eq!(*ctl.reboots.lock().unwrap(), 1, "once the toggle ends, reboot works again");
    }
}
