//! Debug surfaces (ADB, UART root console) and the pure logic around them.
//! The adapter runs `systemctl`, this module only interprets its text and
//! validates client input against a whitelist, exactly like `usb_role.rs`.

use crate::domain::usb_role::UsbRole;
use crate::utils::types::AppError;

/// The two debug surfaces the Settings menu can toggle at runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Surface {
    Adb,
    Uart,
}

impl Surface {
    pub fn as_str(self) -> &'static str {
        match self {
            Surface::Adb => "adb",
            Surface::Uart => "uart",
        }
    }

    /// Exact lowercase match only: the whitelist for client input.
    pub fn parse(s: &str) -> Option<Surface> {
        match s {
            "adb" => Some(Surface::Adb),
            "uart" => Some(Surface::Uart),
            _ => None,
        }
    }
}

pub fn validate_surface(s: &str) -> Result<Surface, AppError> {
    Surface::parse(s).ok_or_else(|| AppError::invalid(format!("'{s}' is not a debug surface (adb or uart)")))
}

/// Both surfaces' live state, as reported by `debug_access_get`/`_set` and
/// consumed by the shared chrome badge. Always built from a fresh
/// `systemctl is-active` query, never cached.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DebugState {
    pub adb: bool,
    pub uart: bool,
}

impl DebugState {
    pub fn get(self, surface: Surface) -> bool {
        match surface {
            Surface::Adb => self.adb,
            Surface::Uart => self.uart,
        }
    }

    pub fn set(&mut self, surface: Surface, enabled: bool) {
        match surface {
            Surface::Adb => self.adb = enabled,
            Surface::Uart => self.uart = enabled,
        }
    }

    /// Drives the shared chrome badge (plan section 6): on while either
    /// surface is enabled, one generic label, not per surface.
    pub fn any_enabled(self) -> bool {
        self.adb || self.uart
    }
}

/// Reads the one line `systemctl is-active <unit>` prints. Only the exact
/// text `active` is enabled; every other value (`inactive`, `failed`,
/// `activating`, `unknown`, empty, garbage) reads as disabled. This is the
/// fail-safe direction the brief requires: a parse oddity must never read as
/// "on".
pub fn parse_active(text: &str) -> bool {
    text.trim() == "active"
}

/// Stable wire-facing codes for every way a debug-access command can be
/// refused (plan section 5.3, surface-renamed per this task's brief).
/// Siblings compare these strings, never the free-text message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DebugAccessErrorCode {
    /// No unlocked Users admin session on this connection.
    NotUnlocked,
    /// The wire command's `confirm` field did not match the surface.
    ConfirmRequired,
    /// `kind` was not `"adb"` or `"uart"`.
    InvalidSurface,
    /// Enabling adb was attempted while the live USB role is not `Device`.
    UsbRoleHost,
    /// The adapter's `systemctl start`/`stop` failed or left the unit in
    /// the wrong state; the surface is left (or returned) disabled.
    ToggleFailed,
    /// A toggle was attempted while another one is already in flight.
    Busy,
    /// `systemctl` itself could not be run.
    Unavailable,
    /// Too many `debug_set` attempts on this connection in the window.
    /// Added for T-D3.1: the wire's `DEBUG_ERROR_CODES` already names
    /// `rate_limited` (plan section 5.3); this variant was missing.
    RateLimited,
}

impl DebugAccessErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            DebugAccessErrorCode::NotUnlocked => "not_unlocked",
            DebugAccessErrorCode::ConfirmRequired => "confirm_required",
            DebugAccessErrorCode::InvalidSurface => "invalid_surface",
            DebugAccessErrorCode::UsbRoleHost => "usb_role_host",
            DebugAccessErrorCode::ToggleFailed => "toggle_failed",
            DebugAccessErrorCode::Busy => "busy",
            DebugAccessErrorCode::Unavailable => "unavailable",
            DebugAccessErrorCode::RateLimited => "rate_limited",
        }
    }
}

/// Maps an adapter failure (`AppError` from `DebugAccessCtl`) to the stable
/// code a caller reports; `Unavailable` keeps its own code, everything else
/// an adapter can fail with is `toggle_failed` (the fail-safe direction is
/// always "off", never a guess about what went wrong).
pub fn toggle_error_code(e: &AppError) -> DebugAccessErrorCode {
    match e.kind {
        crate::utils::types::ErrorKind::Unavailable => DebugAccessErrorCode::Unavailable,
        _ => DebugAccessErrorCode::ToggleFailed,
    }
}

/// A flag shared between `DebugAccessService`'s mutation mutex and
/// `UsbRoleService::reboot` (owner decision, plan section 12: refuse a
/// reboot while a toggle is in flight). Pure state, no I/O, set true for
/// exactly the span the mutation mutex is held and false otherwise; two
/// services reading/writing a plain atomic is simpler than either owning
/// the other's lock.
pub struct InFlight(std::sync::atomic::AtomicBool);

impl InFlight {
    pub fn new() -> InFlight {
        InFlight(std::sync::atomic::AtomicBool::new(false))
    }

    pub fn get(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn set(&self, value: bool) {
        self.0.store(value, std::sync::atomic::Ordering::SeqCst);
    }
}

impl Default for InFlight {
    fn default() -> InFlight {
        InFlight::new()
    }
}

/// Enabling adb needs the live USB role to be `Device`: host mode has no
/// device-mode gadget for adbd to bind to (plan section 4.3). Pure: the
/// caller already queried the role through the existing `UsbRoleCtl` port.
/// Anything that is not confirmed `Device` (including an unknown role)
/// refuses, matching the plan's "if the running role is not Device" wording.
pub fn guard_adb_enable(role: Option<UsbRole>) -> Result<(), DebugAccessErrorCode> {
    if role == Some(UsbRole::Device) {
        Ok(())
    } else {
        Err(DebugAccessErrorCode::UsbRoleHost)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::types::ErrorKind;

    #[test]
    fn surface_parse_is_a_strict_whitelist() {
        assert_eq!(Surface::parse("adb"), Some(Surface::Adb));
        assert_eq!(Surface::parse("uart"), Some(Surface::Uart));
        for bad in ["", "ADB", "Adb", " adb", "adb ", "usb", "adb; reboot", "uart\n", "both"] {
            assert_eq!(Surface::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn validate_surface_matches_parse() {
        assert_eq!(validate_surface("adb").unwrap(), Surface::Adb);
        assert_eq!(validate_surface("uart").unwrap(), Surface::Uart);
        let e = validate_surface("ADB").unwrap_err();
        assert_eq!(e.kind, ErrorKind::Invalid);
    }

    #[test]
    fn as_str_round_trips_through_parse() {
        for s in [Surface::Adb, Surface::Uart] {
            assert_eq!(Surface::parse(s.as_str()), Some(s));
        }
    }

    #[test]
    fn parse_active_only_accepts_the_exact_word() {
        assert!(parse_active("active"));
        assert!(parse_active("  active\n"));
        for bad in ["inactive", "failed", "activating", "unknown", "", "Active", "active "] {
            assert!(bad.trim() == "active" || !parse_active(bad), "{bad:?}");
        }
        // Explicit fail-safe cases the brief calls out by name.
        assert!(!parse_active("inactive"));
        assert!(!parse_active("failed"));
        assert!(!parse_active("activating"));
        assert!(!parse_active(""));
        assert!(!parse_active("garbage\nmore garbage"));
    }

    #[test]
    fn debug_state_get_set_and_any_enabled() {
        let mut s = DebugState::default();
        assert!(!s.any_enabled());
        s.set(Surface::Adb, true);
        assert!(s.get(Surface::Adb));
        assert!(!s.get(Surface::Uart));
        assert!(s.any_enabled());
        s.set(Surface::Adb, false);
        s.set(Surface::Uart, true);
        assert!(!s.get(Surface::Adb));
        assert!(s.get(Surface::Uart));
        assert!(s.any_enabled());
    }

    #[test]
    fn error_codes_are_stable_strings() {
        let pairs = [
            (DebugAccessErrorCode::NotUnlocked, "not_unlocked"),
            (DebugAccessErrorCode::ConfirmRequired, "confirm_required"),
            (DebugAccessErrorCode::InvalidSurface, "invalid_surface"),
            (DebugAccessErrorCode::UsbRoleHost, "usb_role_host"),
            (DebugAccessErrorCode::ToggleFailed, "toggle_failed"),
            (DebugAccessErrorCode::Busy, "busy"),
            (DebugAccessErrorCode::Unavailable, "unavailable"),
            (DebugAccessErrorCode::RateLimited, "rate_limited"),
        ];
        for (code, s) in pairs {
            assert_eq!(code.as_str(), s);
        }
    }

    #[test]
    fn toggle_error_code_keeps_unavailable_and_folds_the_rest_into_toggle_failed() {
        assert_eq!(toggle_error_code(&AppError::unavailable("no systemctl")), DebugAccessErrorCode::Unavailable);
        assert_eq!(toggle_error_code(&AppError::failed("systemctl start x failed: boom")), DebugAccessErrorCode::ToggleFailed);
        assert_eq!(toggle_error_code(&AppError::invalid("bad")), DebugAccessErrorCode::ToggleFailed);
    }

    #[test]
    fn adb_enable_is_refused_unless_the_live_role_is_confirmed_device() {
        assert_eq!(guard_adb_enable(Some(UsbRole::Device)), Ok(()));
        assert_eq!(guard_adb_enable(Some(UsbRole::Host)), Err(DebugAccessErrorCode::UsbRoleHost));
        assert_eq!(guard_adb_enable(None), Err(DebugAccessErrorCode::UsbRoleHost));
    }
}
