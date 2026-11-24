//! Which commands run on the WebSocket reader thread and which are handed to
//! a worker thread. One declared table instead of `thread::spawn` calls
//! scattered over the router arms.

use crate::protocol::ClientCommand;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Execution {
    /// Quick local call: runs on the reader thread.
    Inline,
    /// Blocks for seconds (a remote round trip, an event wait, DHCP) or on the
    /// client's own reply: must not run on the reader thread, or that reply
    /// could never be read.
    Blocking,
}

pub fn execution(cmd: &ClientCommand) -> Execution {
    use ClientCommand as C;
    match cmd {
        // Device1.Pair() blocks on this process's own agent callback getting
        // its WS round trip answered; Connect/Disconnect can take seconds
        // for a real reconnect.
        C::BtPair { .. } | C::BtConnect { .. } | C::BtDisconnect { .. } => Execution::Blocking,
        // Scan and connect wait on supplicant events and DHCP.
        C::WifiScan {} | C::WifiConnect { .. } | C::WifiConnectSaved { .. } => Execution::Blocking,
        // Spawn `superbird-usb-role` or `systemctl`.
        C::UsbRoleGet {} | C::UsbRoleSet { .. } | C::UsbRoleReboot {} => Execution::Blocking,
        // Both spawn `systemctl` (status checks and, for `debug_set`, the
        // enable/disable calls serialized behind the mutation mutex);
        // neither may run on the reader thread, same reasoning as usb_role.
        C::DebugGet {} | C::DebugSet { .. } => Execution::Blocking,
        // The real service runs the device tools and writes the settings
        // file, so all three are Blocking from the start.
        C::DeviceSettingsGet {} | C::DeviceSettingsSet { .. } | C::DeviceReboot {} => Execution::Blocking,
        // Every user_*, group_* and ssh_key_* command is Blocking (plan
        // section 7): the services read and write account files and run the
        // system tools (`unix_chkpwd` verification included, which is slow by
        // design).
        C::UserBootstrapGet {}
        | C::UserUnlock { .. }
        | C::UserLockSession {}
        | C::UserListGet {}
        | C::UserAdd { .. }
        | C::UserRemove { .. }
        | C::UserSetLocked { .. }
        | C::UserSetPassword { .. }
        | C::GroupListGet {}
        | C::UserGroupAdd { .. }
        | C::UserGroupRemove { .. }
        | C::GroupAdd { .. }
        | C::GroupRemove { .. }
        | C::SshKeyListGet { .. }
        | C::SshKeyAdd { .. }
        | C::SshKeyRemove { .. } => Execution::Blocking,
        // Adapter-local D-Bus calls, sysfs, timedatectl and the settings file
        // are fast enough to run inline, including bt_forget (RemoveDevice
        // has no remote round trip) and bt_scan_start/stop.
        C::BtPowerGet {}
        | C::BtSetPower { .. }
        | C::BtScanStart {}
        | C::BtScanStop {}
        | C::BtPairingConfirm { .. }
        | C::BtPairedListGet {}
        | C::BtForget { .. }
        | C::BrightnessGet {}
        | C::BrightnessSet { .. }
        | C::TimezoneGet {}
        | C::TimezoneSet { .. }
        | C::TimezoneListGet {}
        | C::WifiGet {}
        | C::WifiDisconnect {}
        | C::WifiSavedListGet {}
        | C::WifiForget { .. } => Execution::Inline,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> ClientCommand {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn long_running_commands_are_blocking() {
        for j in [
            r#"{"type":"bt_pair","address":"AA"}"#,
            r#"{"type":"bt_connect","address":"AA"}"#,
            r#"{"type":"bt_disconnect","address":"AA"}"#,
            r#"{"type":"usb_role_get"}"#,
            r#"{"type":"usb_role_set","role":"host"}"#,
            r#"{"type":"usb_role_reboot"}"#,
            r#"{"type":"debug_get"}"#,
            r#"{"type":"debug_set","surface":"adb","enabled":true,"confirm":"adb"}"#,
            r#"{"type":"device_settings_get"}"#,
            r#"{"type":"device_settings_set","setting":"ssh","value":"off"}"#,
            r#"{"type":"device_reboot"}"#,
            r#"{"type":"wifi_scan"}"#,
            r#"{"type":"wifi_connect","ssid":"x"}"#,
            r#"{"type":"wifi_connect_saved","ssid":"x"}"#,
            r#"{"type":"user_bootstrap_get"}"#,
            r#"{"type":"user_unlock","username":"ana","password":"x"}"#,
            r#"{"type":"user_lock_session"}"#,
            r#"{"type":"user_list_get"}"#,
            r#"{"type":"user_add","name":"ana"}"#,
            r#"{"type":"user_remove","name":"ana"}"#,
            r#"{"type":"user_set_locked","name":"ana","locked":true}"#,
            r#"{"type":"user_set_password","name":"ana"}"#,
            r#"{"type":"group_list_get"}"#,
            r#"{"type":"user_group_add","name":"ana","group":"adm"}"#,
            r#"{"type":"user_group_remove","name":"ana","group":"adm"}"#,
            r#"{"type":"group_add","name":"crew"}"#,
            r#"{"type":"group_remove","name":"crew"}"#,
            r#"{"type":"ssh_key_list_get","name":"ana"}"#,
            r#"{"type":"ssh_key_add","name":"ana","key":"ssh-ed25519 AAAA"}"#,
            r#"{"type":"ssh_key_remove","name":"ana","fingerprint":"SHA256:x"}"#,
        ] {
            assert_eq!(execution(&parse(j)), Execution::Blocking, "{j}");
        }
    }

    #[test]
    fn quick_commands_stay_inline() {
        for j in [
            r#"{"type":"bt_forget","address":"AA"}"#,
            r#"{"type":"bt_pairing_confirm","accept":true}"#,
            r#"{"type":"brightness_set","value":1}"#,
            r#"{"type":"timezone_set","value":"UTC"}"#,
            r#"{"type":"wifi_get"}"#,
            r#"{"type":"wifi_forget","ssid":"x"}"#,
        ] {
            assert_eq!(execution(&parse(j)), Execution::Inline, "{j}");
        }
    }
}
