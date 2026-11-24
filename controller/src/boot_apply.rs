//! `controller-agent apply-device-settings early|late|failsafe ...`: the boot
//! time apply of the persisted device settings (plan section 4.4).
//!
//! - `early` (unit A, sysinit): load or seed or reseed or repair the JSON,
//!   mirror the mode to the legacy `boot-role` file and create or remove the
//!   `ssh-off` flag that gates `sshd.socket`. File writes only: it takes no
//!   `DeviceCtl` and no unit control at all, so it cannot deadlock the boot
//!   transaction. A test pins that this stays true.
//! - `late` (unit B, multi-user): start exactly the right unit of each pair
//!   (adbd or usb-bind, uart-root or stock getty) and bring ssh in line.
//!   Adb is never started in host mode.
//! - `failsafe early <result>` (ExecStopPost of A): on a failed early run,
//!   file writes only: boot-role device and the ssh-off flag removed, so
//!   ssh and the link stay reachable and adb and uart stay off.
//! - `failsafe late` (unit F, OnFailure of B): mode aware and never
//!   destructive: never edits the JSON, never stops a unit, never starts
//!   adbd or uart-root, never touches ssh, never starts usb-bind in host mode.
//!
//! None of this loads `controller.toml`: the caller builds the adapters from
//! `DeviceSettingsConfig::default()`, whose values are the compiled-in
//! constants of `domain::device_settings` (a test pins them to the shipped
//! config), so a stale or unknown-key config cannot stop the boot.
//!
//! Exit status: 0 on success, 1 when the unit's own goal failed (early could
//! not write its files; late could not bring up a link or a console;
//! failsafe late could not start what it needed), 64 for a usage error.

use crate::domain::device_settings::{self as ds, DeviceSettings, UsbRole};
use crate::ports::{DeviceCtl, DeviceSettingsStore};
use crate::utils::types::AppError;
use log::{error, info, warn};

pub const EXIT_OK: i32 = 0;
pub const EXIT_FAILED: i32 = 1;
pub const EXIT_USAGE: i32 = 64;

/// The units the boot apply starts or probes by role; the adapter maps them
/// to the configured names, so no unit name text passes through here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Unit {
    Adbd,
    UsbBind,
    UartRoot,
    Getty,
}

/// Unit probing and starting for the late and failsafe-late paths, on top of
/// `DeviceCtl`.
pub trait BootCtl: DeviceCtl {
    /// `systemctl is-active`; any failure reads as not active.
    fn is_active(&self, unit: Unit) -> bool;
    fn start(&self, unit: Unit) -> Result<(), AppError>;
}

/// The plain file writes of the early path.
pub trait BootFiles {
    /// Writes the legacy `boot-role` mirror (`host` or `device` plus newline).
    fn write_boot_role(&self, role: UsbRole) -> Result<(), AppError>;
    /// Creates (present) or removes (absent) the `ssh-off` flag.
    fn set_ssh_off_flag(&self, present: bool) -> Result<(), AppError>;
    /// Marks a clean early apply (gates the controller's startup fallback).
    fn mark_applied(&self) -> Result<(), AppError>;
}

/// The settings store plus the read-only mode peek the failsafe unit needs.
pub trait BootStore: DeviceSettingsStore {
    /// Persisted mode, device when missing, unreadable or corrupt; never writes.
    fn peek_mode(&self) -> UsbRole;
}

/// The parsed command line after `apply-device-settings`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Early,
    Late,
    /// `$SERVICE_RESULT` of unit A; absent counts as a failure.
    FailsafeEarly(Option<String>),
    FailsafeLate,
}

pub const USAGE: &str = "usage: controller-agent apply-device-settings early | late | failsafe early [<service-result>] | failsafe late";

/// `args` is everything after `apply-device-settings`.
pub fn parse_args(args: &[String]) -> Result<Command, String> {
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    match a.as_slice() {
        ["early"] => Ok(Command::Early),
        ["late"] => Ok(Command::Late),
        ["failsafe", "early"] => Ok(Command::FailsafeEarly(None)),
        ["failsafe", "early", result] => Ok(Command::FailsafeEarly(Some((*result).to_string()))),
        ["failsafe", "late"] => Ok(Command::FailsafeLate),
        _ => Err(USAGE.to_string()),
    }
}

fn log_notes(notes: &[ds::LoadNote]) {
    for n in notes {
        if n.is_warn() {
            warn!("{}", n.message());
        } else {
            info!("{}", n.message());
        }
    }
}

// early-path:begin

/// Unit A. Returns the exit status.
pub fn early(store: &dyn DeviceSettingsStore, files: &dyn BootFiles) -> i32 {
    let loaded = store.load();
    log_notes(&loaded.notes);
    let s = loaded.settings;
    let mut ok = true;
    if let Err(e) = files.write_boot_role(s.mode()) {
        error!("device-settings early: boot-role not written: {e}");
        ok = false;
    }
    if let Err(e) = files.set_ssh_off_flag(ds::ssh_off_flag_wanted(&s)) {
        error!("device-settings early: ssh-off flag not updated: {e}");
        ok = false;
    }
    if ok {
        if let Err(e) = files.mark_applied() {
            warn!("device-settings early: applied marker not written: {e}");
        }
        info!(
            "device-settings early: mode={} ssh={} adb={} uart={}",
            s.mode().as_str(),
            s.ssh(),
            s.adb(),
            s.uart()
        );
        EXIT_OK
    } else {
        EXIT_FAILED
    }
}

/// ExecStopPost of unit A. A clean run (and the normal stop at shutdown)
/// passes `success` and is a no-op; anything else, including a missing
/// result, takes the failure branch: boot-role to device and ssh reachable.
/// Adb and uart stay off because nothing starts them.
pub fn failsafe_early(result: Option<&str>, files: &dyn BootFiles) -> i32 {
    if result == Some("success") {
        return EXIT_OK;
    }
    warn!("device-settings failsafe early: unit A result {:?}, restoring boot-role device and removing the ssh-off flag", result.unwrap_or("unknown"));
    let mut ok = true;
    if let Err(e) = files.write_boot_role(ds::FAILSAFE_BOOT_ROLE) {
        error!("device-settings failsafe early: boot-role not written: {e}");
        ok = false;
    }
    if ds::FAILSAFE_SSH_REACHABLE {
        if let Err(e) = files.set_ssh_off_flag(false) {
            error!("device-settings failsafe early: ssh-off flag not removed: {e}");
            ok = false;
        }
    }
    if ok {
        EXIT_OK
    } else {
        EXIT_FAILED
    }
}

// early-path:end

/// Unit B. Re-reads the file itself (the loader re-runs the invariant) and
/// applies it live by starting units; it does not trust that A already ran.
/// A failure of the ssh step or of a mode switch is logged and is not a
/// failure of the unit; only a missing link or console is.
pub fn late(store: &dyn DeviceSettingsStore, ctl: &dyn BootCtl) -> i32 {
    let loaded = store.load();
    log_notes(&loaded.notes);
    let s: DeviceSettings = loaded.settings;
    info!("device-settings late: applying mode={} ssh={} adb={} uart={}", s.mode().as_str(), s.ssh(), s.adb(), s.uart());
    let mut broken = false;

    // 1. Mode: only switch the role when it differs from the live one.
    match ctl.usb_status() {
        Ok(st) if st.role == Some(s.mode()) => {}
        Ok(_) => {
            let switched = if s.mode() == UsbRole::Host { ctl.to_host() } else { ctl.to_device() };
            if let Err(e) = switched {
                warn!("device-settings late: switching the role to {} failed: {e}", s.mode().as_str());
            }
        }
        Err(e) => warn!("device-settings late: usb role status unreadable: {e}"),
    }

    // 2. Gadget side, device mode only: adbd is never started in host role.
    if s.mode() == UsbRole::Device {
        let mut link = false;
        if s.adb() {
            match ctl.set_live(ds::Setting::Adb, true) {
                Ok(()) => link = true,
                // The restrictive direction is the fallback: an NCM-only link.
                Err(e) => warn!("device-settings late: starting adbd failed: {e}, falling back to usb-bind"),
            }
        }
        if !link {
            match ctl.set_live(ds::Setting::Adb, false) {
                Ok(()) => link = true,
                Err(e) => error!("device-settings late: starting usb-bind failed: {e}"),
            }
        }
        broken |= !link;
    }

    // 3. Console: uart-root when on, the stock getty otherwise.
    let mut console = false;
    if s.uart() {
        match ctl.set_live(ds::Setting::Uart, true) {
            Ok(()) => console = true,
            Err(e) => warn!("device-settings late: starting the uart root console failed: {e}, falling back to the stock getty"),
        }
    }
    if !console {
        match ctl.set_live(ds::Setting::Uart, false) {
            Ok(()) => console = true,
            Err(e) => error!("device-settings late: starting the stock getty failed: {e}"),
        }
    }
    broken |= !console;

    // 4. ssh. Off is always (re)applied: it restores the flag and ends any
    // instance. On only starts the socket when it is not already running.
    if s.ssh() {
        if !ctl.is_on(ds::Setting::Ssh).unwrap_or(false) {
            if let Err(e) = ctl.set_live(ds::Setting::Ssh, true) {
                warn!("device-settings late: starting sshd.socket failed: {e}");
            }
        }
    } else if let Err(e) = ctl.set_live(ds::Setting::Ssh, false) {
        warn!("device-settings late: closing ssh failed: {e}");
    }

    if broken {
        EXIT_FAILED
    } else {
        EXIT_OK
    }
}

/// Unit F. See the module comment for the rules.
pub fn failsafe_late(store: &dyn BootStore, ctl: &dyn BootCtl) -> i32 {
    let mode = store.peek_mode();
    let mut ok = true;

    // Gadget side: only in persisted device mode, only when the role is not
    // host (an unreadable status is not host), only when nothing holds it.
    if mode == UsbRole::Device {
        let host_now = matches!(ctl.usb_status(), Ok(st) if st.role == Some(UsbRole::Host));
        if host_now {
            info!("device-settings failsafe late: role is host, leaving the gadget alone");
        } else if ctl.is_active(Unit::Adbd) || ctl.is_active(Unit::UsbBind) {
            info!("device-settings failsafe late: a gadget unit is already active, leaving it");
        } else {
            warn!("device-settings failsafe late: no gadget unit active, starting usb-bind (a persisted adb that failed to start stays off)");
            if let Err(e) = ctl.start(Unit::UsbBind) {
                error!("device-settings failsafe late: starting usb-bind failed: {e}");
                ok = false;
            }
        }
    } else {
        info!("device-settings failsafe late: persisted mode is host, starting no gadget unit");
    }

    // Console side: a running surface (uart-root or stock getty) is left alone.
    if ctl.is_active(Unit::UartRoot) || ctl.is_active(Unit::Getty) {
        info!("device-settings failsafe late: a console unit is already active, leaving it");
    } else {
        warn!("device-settings failsafe late: no console unit active, starting the stock getty (a persisted uart that failed to start stays off)");
        if let Err(e) = ctl.start(Unit::Getty) {
            error!("device-settings failsafe late: starting the stock getty failed: {e}");
            ok = false;
        }
    }

    if ok {
        EXIT_OK
    } else {
        EXIT_FAILED
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_cli_matches_the_unit_files() {
        assert_eq!(parse_args(&v(&["early"])), Ok(Command::Early));
        assert_eq!(parse_args(&v(&["late"])), Ok(Command::Late));
        assert_eq!(parse_args(&v(&["failsafe", "late"])), Ok(Command::FailsafeLate));
        assert_eq!(parse_args(&v(&["failsafe", "early", "success"])), Ok(Command::FailsafeEarly(Some("success".into()))));
        assert_eq!(parse_args(&v(&["failsafe", "early", "exit-code"])), Ok(Command::FailsafeEarly(Some("exit-code".into()))));
        assert_eq!(parse_args(&v(&["failsafe", "early"])), Ok(Command::FailsafeEarly(None)));
    }

    #[test]
    fn anything_else_is_a_usage_error() {
        for bad in [&[][..], &["bogus"], &["early", "x"], &["late", "x"], &["failsafe"], &["failsafe", "mid"], &["failsafe", "late", "x"], &["failsafe", "early", "a", "b"], &["Early"]] {
            assert_eq!(parse_args(&v(bad)), Err(USAGE.to_string()), "{bad:?}");
        }
    }
}
