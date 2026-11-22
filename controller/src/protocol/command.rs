//! Inbound wire commands (client to controller). Serde only.

use serde::Deserialize;

/// A string that must never reach a log line: its `Debug` output is fixed.
/// `ClientCommand` derives `Debug`, and a password-carrying variant must
/// not leak through a stray `{:?}`.
#[derive(Deserialize, Clone, Default)]
#[serde(transparent)]
pub struct Secret(String);

impl std::fmt::Debug for Secret {
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
}
