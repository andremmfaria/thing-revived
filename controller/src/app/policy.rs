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
            r#"{"type":"wifi_scan"}"#,
            r#"{"type":"wifi_connect","ssid":"x"}"#,
            r#"{"type":"wifi_connect_saved","ssid":"x"}"#,
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
