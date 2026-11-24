//! Inbound wire commands (client to controller). Serde only.

use serde::{Deserialize, Serialize};

/// A string that must never reach a log line: its `Debug` output is fixed.
/// `ClientCommand` derives `Debug`, and a password-carrying variant must
/// not leak through a stray `{:?}`. `Display` is redacted the same way.
/// `Serialize` writes the real value: it exists only so the two reveal
/// events (`user_bootstrap_secret`, `user_secret`) can carry a one-time password, and
/// no other event may hold a `Secret`.
#[derive(Deserialize, Serialize, Clone, Default)]
#[serde(transparent)]
pub struct Secret(String);

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<redacted>")
    }
}

impl std::fmt::Display for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<redacted>")
    }
}

impl Secret {
    /// The raw value. Call sites must never pass it to a log macro.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

/// Inbound commands a WS client may send. Anything not matching a known
/// `type`/shape is ignored (same permissive-ignore convention
/// the terminal agent in daemon/ uses for its protocol).
#[derive(Deserialize, Clone, Debug)]
#[serde(tag = "type")]
pub enum ClientCommand {
    #[serde(rename = "bt_power_get")]
    BtPowerGet {},
    #[serde(rename = "bt_set_power")]
    BtSetPower { powered: bool },
    #[serde(rename = "bt_scan_start")]
    BtScanStart {},
    #[serde(rename = "bt_scan_stop")]
    BtScanStop {},
    #[serde(rename = "bt_pair")]
    BtPair { address: String },
    #[serde(rename = "bt_pairing_confirm")]
    BtPairingConfirm { accept: bool },
    #[serde(rename = "bt_paired_list_get")]
    BtPairedListGet {},
    #[serde(rename = "bt_connect")]
    BtConnect { address: String },
    #[serde(rename = "bt_disconnect")]
    BtDisconnect { address: String },
    #[serde(rename = "bt_forget")]
    BtForget { address: String },
    #[serde(rename = "brightness_get")]
    BrightnessGet {},
    #[serde(rename = "brightness_set")]
    BrightnessSet { value: u32 },
    #[serde(rename = "timezone_get")]
    TimezoneGet {},
    #[serde(rename = "timezone_set")]
    TimezoneSet { value: String },
    #[serde(rename = "timezone_list_get")]
    TimezoneListGet {},
    /// Reply: `usb_role`, or `usb_role_error`.
    #[serde(rename = "usb_role_get")]
    UsbRoleGet {},
    /// Persists the boot role (applies from the next boot, never a live
    /// switch). `role` is a plain string so a bad value gets a
    /// `usb_role_error` (`invalid_role`) instead of being dropped as
    /// unparseable; the service checks it against `host`/`device`.
    #[serde(rename = "usb_role_set")]
    UsbRoleSet { role: String },
    /// Reboots the device. Reply: `usb_role_rebooting`, or `usb_role_error`.
    #[serde(rename = "usb_role_reboot")]
    UsbRoleReboot {},
    // --- Debug access (docs/research/debug-toggles-plan.md section 5.3,
    // T-D2.3 delegation). Named `debug_get`/`debug_set` with a `surface`
    // field rather than the plan's `debug_access_get`/`debug_access_set`/
    // `kind`: the task brief fixed this shorter family name and field name
    // explicitly, so it is kept even though it reads differently from the
    // plan text; the error codes and gate order it describes are
    // unchanged. ---
    /// Reply: `debug_state`, or `debug_error`.
    #[serde(rename = "debug_get")]
    DebugGet {},
    /// Toggles the adb or UART-root debug surface. `surface` is `"adb"` or
    /// `"uart"`; anything else is `invalid_surface`. `confirm` must equal
    /// `surface` exactly (the hold-to-confirm gesture is the real consent
    /// per docs/research/debug-toggles-plan.md section 12; this field is
    /// the server-side check only, never a second typed prompt) or the
    /// reply is `confirm_required`. Requires an unlocked Users admin
    /// session (`not_unlocked` otherwise). Reply: `debug_state`
    /// (broadcast), or `debug_error`.
    #[serde(rename = "debug_set")]
    DebugSet { surface: String, enabled: bool, #[serde(default)] confirm: String },
    // --- Device settings (docs/research/device-settings-plan.md section
    // 3.2). Added next to the usb_role and debug families, which the
    // wave 3 cutover deletes. ---
    /// Reads the live and persisted device settings. Not gated by the
    /// admin session: the status badge needs the live adb and uart state
    /// while locked. Reply: `device_settings`, or `device_settings_error`.
    #[serde(rename = "device_settings_get")]
    DeviceSettingsGet {},
    /// Changes one setting. `setting` is `mode`, `ssh`, `adb` or `uart`;
    /// `value` is `host`/`device` for `mode` and `on`/`off` otherwise.
    /// Both stay plain strings so a bad value is answered with
    /// `device_settings_error` (`invalid_setting`, `invalid_value`)
    /// instead of failing to parse. Needs an unlocked admin session.
    #[serde(rename = "device_settings_set")]
    DeviceSettingsSet { setting: String, value: String },
    /// Reboots the device. Needs an unlocked admin session. Reply:
    /// `device_rebooting`, or `device_settings_error`.
    #[serde(rename = "device_reboot")]
    DeviceReboot {},

    #[serde(rename = "wifi_get")]
    WifiGet {},
    #[serde(rename = "wifi_scan")]
    WifiScan {},
    /// `password` is empty for open networks. Security is taken from the
    /// agent's own last scan, not from the client.
    #[serde(rename = "wifi_connect")]
    WifiConnect { ssid: String, #[serde(default)] password: Secret },
    #[serde(rename = "wifi_connect_saved")]
    WifiConnectSaved { ssid: String },
    #[serde(rename = "wifi_disconnect")]
    WifiDisconnect {},
    #[serde(rename = "wifi_saved_list_get")]
    WifiSavedListGet {},
    #[serde(rename = "wifi_forget")]
    WifiForget { ssid: String },

    // --- Users (docs/research/user-management-plan.md section 8, amended by
    // docs/research/user-management-plan-delta-password-auth.md section 5.6) ---
    /// The frontend is ready and asks for the first-boot state. Reply:
    /// `user_bootstrap`, plus `user_bootstrap_secret` while the password is
    /// showing. Generation and acknowledgement are never wire commands: they
    /// are the physical M button only.
    #[serde(rename = "user_bootstrap_get")]
    UserBootstrapGet {},
    /// Unlock an admin session on this connection. Reply: `user_session`,
    /// or `user_error` (`bad_password`, `rate_limited`, `no_password`).
    #[serde(rename = "user_unlock")]
    UserUnlock { username: String, password: Secret },
    /// End the admin session of this connection. Reply: `user_session`.
    #[serde(rename = "user_lock_session")]
    UserLockSession {},
    #[serde(rename = "user_list_get")]
    UserListGet {},
    /// An empty `password` means: generate one (reply `user_secret`).
    #[serde(rename = "user_add")]
    UserAdd { name: String, #[serde(default)] password: Secret, #[serde(default)] admin: bool },
    /// `confirm` must echo the name.
    #[serde(rename = "user_remove")]
    UserRemove { name: String, #[serde(default)] keep_home: bool, #[serde(default)] confirm: String },
    /// `confirm` must echo the name.
    #[serde(rename = "user_set_locked")]
    UserSetLocked { name: String, locked: bool, #[serde(default)] confirm: String },
    /// An empty `password` means: generate one. `current_password` is for the
    /// caller's own account only; `confirm` echoes the name when resetting
    /// another user.
    #[serde(rename = "user_set_password")]
    UserSetPassword {
        name: String,
        #[serde(default)]
        password: Secret,
        #[serde(default)]
        current_password: Secret,
        #[serde(default)]
        confirm: String,
    },
    #[serde(rename = "group_list_get")]
    GroupListGet {},
    /// `confirm` is required when `group` is `sudo`.
    #[serde(rename = "user_group_add")]
    UserGroupAdd { name: String, group: String, #[serde(default)] confirm: String },
    #[serde(rename = "user_group_remove")]
    UserGroupRemove { name: String, group: String },
    #[serde(rename = "group_add")]
    GroupAdd { name: String },
    /// `confirm` must echo the group name.
    #[serde(rename = "group_remove")]
    GroupRemove { name: String, #[serde(default)] confirm: String },
    #[serde(rename = "ssh_key_list_get")]
    SshKeyListGet { name: String },
    /// `key` is one public key line (`ssh-ed25519 AAAA... comment`).
    #[serde(rename = "ssh_key_add")]
    SshKeyAdd { name: String, key: String },
    /// `confirm` must echo the fingerprint.
    #[serde(rename = "ssh_key_remove")]
    SshKeyRemove { name: String, fingerprint: String, #[serde(default)] confirm: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wifi_commands_parse() {
        let c: ClientCommand = serde_json::from_str(r#"{"type":"wifi_scan"}"#).unwrap();
        assert!(matches!(c, ClientCommand::WifiScan {}));
        let c: ClientCommand = serde_json::from_str(r#"{"type":"wifi_forget","ssid":"Home"}"#).unwrap();
        assert!(matches!(c, ClientCommand::WifiForget { ssid } if ssid == "Home"));
        let c: ClientCommand = serde_json::from_str(r#"{"type":"wifi_connect","ssid":"Home","password":"hunter2hunter2"}"#).unwrap();
        match c {
            ClientCommand::WifiConnect { ssid, password } => {
                assert_eq!(ssid, "Home");
                assert_eq!(password.expose(), "hunter2hunter2");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn wifi_connect_password_defaults_to_empty_for_open_networks() {
        let c: ClientCommand = serde_json::from_str(r#"{"type":"wifi_connect","ssid":"Cafe"}"#).unwrap();
        assert!(matches!(c, ClientCommand::WifiConnect { password, .. } if password.expose().is_empty()));
    }

    #[test]
    fn wifi_connect_missing_ssid_and_unknown_types_are_rejected() {
        assert!(serde_json::from_str::<ClientCommand>(r#"{"type":"wifi_connect","password":"x"}"#).is_err());
        assert!(serde_json::from_str::<ClientCommand>(r#"{"type":"wifi_nope"}"#).is_err());
    }

    #[test]
    fn password_never_appears_in_debug_output() {
        let c: ClientCommand = serde_json::from_str(r#"{"type":"wifi_connect","ssid":"Home","password":"hunter2hunter2"}"#).unwrap();
        let dbg = format!("{c:?}");
        assert!(!dbg.contains("hunter2"), "{dbg}");
        assert!(dbg.contains("<redacted>"));
    }

    #[test]
    fn user_bootstrap_get_parses_and_reveal_and_ack_are_gone() {
        let c: ClientCommand = serde_json::from_str(r#"{"type":"user_bootstrap_get"}"#).unwrap();
        assert!(matches!(c, ClientCommand::UserBootstrapGet {}));
        assert!(serde_json::from_str::<ClientCommand>(r#"{"type":"user_bootstrap_reveal"}"#).is_err());
        assert!(serde_json::from_str::<ClientCommand>(r#"{"type":"user_bootstrap_ack"}"#).is_err());
        assert!(serde_json::from_str::<ClientCommand>(r#"{"type":"user_bootstrap_ack","last4":"k7Qz"}"#).is_err());
    }

    #[test]
    fn user_command_passwords_are_redacted_and_optional_fields_default() {
        let c: ClientCommand = serde_json::from_str(r#"{"type":"user_add","name":"ana"}"#).unwrap();
        match &c {
            ClientCommand::UserAdd { name, password, admin } => {
                assert_eq!(name, "ana");
                assert!(password.expose().is_empty());
                assert!(!admin);
            }
            other => panic!("wrong variant: {other:?}"),
        }
        let c: ClientCommand =
            serde_json::from_str(r#"{"type":"user_unlock","username":"ana","password":"hunter2hunter2"}"#).unwrap();
        let dbg = format!("{c:?}");
        assert!(!dbg.contains("hunter2"), "{dbg}");
        assert!(dbg.contains("<redacted>"));
        let c: ClientCommand = serde_json::from_str(r#"{"type":"user_remove","name":"ana"}"#).unwrap();
        assert!(matches!(c, ClientCommand::UserRemove { keep_home: false, confirm, .. } if confirm.is_empty()));
        assert!(serde_json::from_str::<ClientCommand>(r#"{"type":"user_unlock","username":"ana"}"#).is_err());
    }

    #[test]
    fn usb_role_commands_parse_and_a_bad_shape_is_rejected() {
        let c: ClientCommand = serde_json::from_str(r#"{"type":"usb_role_set","role":"host"}"#).unwrap();
        assert!(matches!(c, ClientCommand::UsbRoleSet { role } if role == "host"));
        assert!(matches!(serde_json::from_str::<ClientCommand>(r#"{"type":"usb_role_get"}"#).unwrap(), ClientCommand::UsbRoleGet {}));
        assert!(matches!(serde_json::from_str::<ClientCommand>(r#"{"type":"usb_role_reboot"}"#).unwrap(), ClientCommand::UsbRoleReboot {}));
        assert!(serde_json::from_str::<ClientCommand>(r#"{"type":"usb_role_set","role":true}"#).is_err());
    }

    #[test]
    fn device_settings_commands_parse_and_a_bad_shape_is_rejected() {
        assert!(matches!(serde_json::from_str::<ClientCommand>(r#"{"type":"device_settings_get"}"#).unwrap(), ClientCommand::DeviceSettingsGet {}));
        assert!(matches!(serde_json::from_str::<ClientCommand>(r#"{"type":"device_reboot"}"#).unwrap(), ClientCommand::DeviceReboot {}));
        let c: ClientCommand = serde_json::from_str(r#"{"type":"device_settings_set","setting":"ssh","value":"off"}"#).unwrap();
        assert!(matches!(c, ClientCommand::DeviceSettingsSet { setting, value } if setting == "ssh" && value == "off"));
        assert!(serde_json::from_str::<ClientCommand>(r#"{"type":"device_settings_set","setting":"ssh"}"#).is_err());
        assert!(serde_json::from_str::<ClientCommand>(r#"{"type":"device_settings_set","setting":"ssh","value":true}"#).is_err());
    }

    #[test]
    fn debug_commands_parse_and_a_missing_confirm_defaults_to_empty() {
        assert!(matches!(serde_json::from_str::<ClientCommand>(r#"{"type":"debug_get"}"#).unwrap(), ClientCommand::DebugGet {}));
        let c: ClientCommand = serde_json::from_str(r#"{"type":"debug_set","surface":"adb","enabled":true,"confirm":"adb"}"#).unwrap();
        assert!(matches!(c, ClientCommand::DebugSet { surface, enabled: true, confirm } if surface == "adb" && confirm == "adb"));
        let c: ClientCommand = serde_json::from_str(r#"{"type":"debug_set","surface":"uart","enabled":false}"#).unwrap();
        assert!(matches!(c, ClientCommand::DebugSet { surface, enabled: false, confirm } if surface == "uart" && confirm.is_empty()));
        assert!(serde_json::from_str::<ClientCommand>(r#"{"type":"debug_set","enabled":true}"#).is_err());
    }

    #[test]
    fn secret_display_and_debug_never_show_the_value() {
        let s: Secret = serde_json::from_str(r#""topsecretvalue""#).unwrap();
        assert_eq!(format!("{s}"), "<redacted>");
        assert_eq!(format!("{s:?}"), "<redacted>");
        assert_eq!(format!("{s:>40}"), "<redacted>");
    }
}
