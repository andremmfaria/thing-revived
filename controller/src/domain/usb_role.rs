//! USB role values and the parser for `superbird-usb-role status`. Pure: the
//! adapter runs the tool, this module only interprets its text. A client
//! supplied role string must pass `validate_role` before it can become a
//! command argument.

use crate::utils::types::AppError;

/// The two roles the USB-C port can boot in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsbRole {
    /// Gadget: the USB link to a PC (SSH, adb).
    Device,
    /// Host: USB Wi-Fi, Ethernet and storage adapters through a dock.
    Host,
}

impl UsbRole {
    pub fn as_str(self) -> &'static str {
        match self {
            UsbRole::Device => "device",
            UsbRole::Host => "host",
        }
    }

    /// Exact lowercase match only: the whitelist for client input.
    pub fn parse(s: &str) -> Option<UsbRole> {
        match s {
            "device" => Some(UsbRole::Device),
            "host" => Some(UsbRole::Host),
            _ => None,
        }
    }
}

/// What `status` reported. A role the tool printed that is neither `host`
/// nor `device` (for example an unset boot role) reads as `None`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UsbRoleStatus {
    /// The role the port is in right now (`role:` line).
    pub role: Option<UsbRole>,
    /// The role persisted for the next boot (`boot role:` line).
    pub boot_role: Option<UsbRole>,
}

/// Reads `role: ...` and `boot role: ...` lines; every other line (such as
/// `revert:`) and any malformed line is ignored, a missing field is `None`.
pub fn parse_status(text: &str) -> UsbRoleStatus {
    let mut status = UsbRoleStatus::default();
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else { continue };
        let value = UsbRole::parse(&value.trim().to_ascii_lowercase());
        match key.trim().to_ascii_lowercase().as_str() {
            "role" => status.role = value,
            "boot role" => status.boot_role = value,
            _ => {}
        }
    }
    status
}

pub fn validate_role(role: &str) -> Result<UsbRole, AppError> {
    UsbRole::parse(role).ok_or_else(|| AppError::invalid(format!("'{role}' is not a USB role (device or host)")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::types::ErrorKind;

    #[test]
    fn parses_the_documented_status_output() {
        let s = parse_status("role: device\nrevert: none\nboot role: host\n");
        assert_eq!(s, UsbRoleStatus { role: Some(UsbRole::Device), boot_role: Some(UsbRole::Host) });
    }

    #[test]
    fn host_role_with_a_pending_revert() {
        let s = parse_status("role: host\nrevert: in 287s\nboot role: device\n");
        assert_eq!(s, UsbRoleStatus { role: Some(UsbRole::Host), boot_role: Some(UsbRole::Device) });
    }

    #[test]
    fn unknown_lines_and_garbage_are_ignored() {
        let s = parse_status("hello\n\n: : :\nrevert: none\nudc: ff400000.usb\nrole: host\nrole host\n");
        assert_eq!(s, UsbRoleStatus { role: Some(UsbRole::Host), boot_role: None });
    }

    #[test]
    fn missing_or_unrecognised_fields_become_none() {
        assert_eq!(parse_status(""), UsbRoleStatus::default());
        let s = parse_status("role: otg\nboot role: unset\n");
        assert_eq!(s, UsbRoleStatus::default());
        let s = parse_status("boot role: none\nrole: device\n");
        assert_eq!(s, UsbRoleStatus { role: Some(UsbRole::Device), boot_role: None });
    }

    #[test]
    fn keys_and_values_tolerate_case_and_spacing() {
        let s = parse_status("  Role :  HOST  \r\nBoot Role: Device\r\n");
        assert_eq!(s, UsbRoleStatus { role: Some(UsbRole::Host), boot_role: Some(UsbRole::Device) });
    }

    #[test]
    fn validate_is_a_strict_whitelist() {
        assert_eq!(validate_role("host").unwrap(), UsbRole::Host);
        assert_eq!(validate_role("device").unwrap(), UsbRole::Device);
        for bad in ["", "Host", "DEVICE", " host", "host ", "clear", "host; reboot", "--no-revert", "host\n", "to-host"] {
            let e = validate_role(bad).unwrap_err();
            assert_eq!(e.kind, ErrorKind::Invalid, "{bad:?}");
        }
    }
}
