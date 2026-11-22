//! Golden wire test: every `BridgeEvent` serializes byte-for-byte to its
//! committed fixture and every `ClientCommand` fixture parses to the
//! expected variant. Fixtures live in config/protocol/controller/ and are shared
//! with the frontend tests (see config/protocol/controller/README.md).

use controller_agent::protocol::{BridgeEvent, ClientCommand, PairedDeviceWire, Secret, WifiNetworkWire, WifiSavedWire};
use std::path::PathBuf;

fn fixture(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../config/protocol/controller").join(format!("{name}.json"));
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    // Exactly one trailing newline is part of the file convention, not of the message.
    raw.strip_suffix('\n').unwrap_or_else(|| panic!("{} must end with a newline", path.display())).to_string()
}

/// Synthetic first-boot password used by the fixtures and the redaction tests.
const SYNTHETIC_PASSWORD: &str = "Xe4mH7pQ9tLwZc3K";

fn secret(value: &str) -> Secret {
    serde_json::from_str(&serde_json::to_string(value).unwrap()).unwrap()
}

fn events() -> Vec<(&'static str, BridgeEvent)> {
    let paired = |address: &str, name: Option<&str>, connected: bool| PairedDeviceWire {
        address: address.into(),
        name: name.map(Into::into),
        connected,
    };
    vec![
        ("event.button", BridgeEvent::Button { code: 50, value: 1 }),
        ("event.knob", BridgeEvent::Knob { delta: -1 }),
        ("event.bt_power", BridgeEvent::BtPower { powered: true }),
        ("event.bt_error", BridgeEvent::BtError { context: "bt_power_get".into(), message: "Bluetooth unavailable".into() }),
        ("event.bt_scan_started", BridgeEvent::BtScanStarted { duration_secs: 150 }),
        ("event.bt_scan_stopped", BridgeEvent::BtScanStopped {}),
        ("event.bt_device_found", BridgeEvent::BtDeviceFound { address: "AA:BB:CC:DD:EE:FF".into(), name: Some("Keyboard".into()) }),
        ("event.bt_device_found.unnamed", BridgeEvent::BtDeviceFound { address: "AA:BB:CC:DD:EE:FF".into(), name: None }),
        ("event.bt_pairing_confirm_request", BridgeEvent::BtPairingConfirmRequest { address: "AA:BB:CC:DD:EE:FF".into(), passkey: 123456 }),
        ("event.bt_pairing_passkey_display", BridgeEvent::BtPairingPasskeyDisplay { address: "AA:BB:CC:DD:EE:FF".into(), passkey: 654321 }),
        ("event.bt_pair_result", BridgeEvent::BtPairResult { address: "AA:BB:CC:DD:EE:FF".into(), success: true, error: None }),
        (
            "event.bt_pair_result.failed",
            BridgeEvent::BtPairResult { address: "AA:BB:CC:DD:EE:FF".into(), success: false, error: Some("confirmation timed out".into()) },
        ),
        (
            "event.bt_paired_list",
            BridgeEvent::BtPairedList {
                devices: vec![paired("AA:BB:CC:DD:EE:FF", Some("Keyboard"), true), paired("11:22:33:44:55:66", None, false)],
            },
        ),
        ("event.brightness", BridgeEvent::Brightness { value: 80, max: 160 }),
        ("event.timezone", BridgeEvent::Timezone { value: "Europe/Lisbon".into() }),
        ("event.timezone_list", BridgeEvent::TimezoneList { zones: vec!["Europe/Lisbon".into(), "UTC".into()] }),
        (
            "event.settings_error",
            BridgeEvent::SettingsError { context: "brightness_set".into(), message: "brightness 200 out of range 0..=160".into() },
        ),
        ("event.wifi_available", BridgeEvent::WifiAvailable { available: true, interface: Some("wlan0".into()) }),
        ("event.wifi_available.none", BridgeEvent::WifiAvailable { available: false, interface: None }),
        (
            "event.wifi_status",
            BridgeEvent::WifiStatus {
                state: "connected".into(),
                ssid: Some("Home".into()),
                ip: Some("192.168.1.9".into()),
                interface: "wlan0".into(),
                signal_pct: Some(80),
                ethernet_up: true,
            },
        ),
        (
            "event.wifi_status.disconnected",
            BridgeEvent::WifiStatus {
                state: "disconnected".into(),
                ssid: None,
                ip: None,
                interface: "wlan0".into(),
                signal_pct: None,
                ethernet_up: false,
            },
        ),
        ("event.wifi_scan_started", BridgeEvent::WifiScanStarted {}),
        (
            "event.wifi_scan_results",
            BridgeEvent::WifiScanResults {
                networks: vec![
                    WifiNetworkWire { ssid: "Home".into(), signal_pct: 80, signal_dbm: -55, security: "wpa2".into(), supported: true },
                    WifiNetworkWire { ssid: "Legacy".into(), signal_pct: 40, signal_dbm: -75, security: "wep".into(), supported: false },
                ],
            },
        ),
        ("event.wifi_connecting", BridgeEvent::WifiConnecting { ssid: "Home".into() }),
        ("event.wifi_connect_result", BridgeEvent::WifiConnectResult { ssid: "Home".into(), success: true, error: None, message: None }),
        (
            "event.wifi_connect_result.failed",
            BridgeEvent::WifiConnectResult {
                ssid: "Home".into(),
                success: false,
                error: Some("wrong_password".into()),
                message: Some("Wrong password".into()),
            },
        ),
        (
            "event.wifi_saved_list",
            BridgeEvent::WifiSavedList {
                networks: vec![
                    WifiSavedWire { ssid: "Home".into(), current: true },
                    WifiSavedWire { ssid: "Office".into(), current: false },
                ],
            },
        ),
        ("event.wifi_error", BridgeEvent::WifiError { context: "wifi_scan".into(), message: "no wireless interface".into() }),
        (
            "event.user_bootstrap",
            BridgeEvent::UserBootstrap { pending: true, username: "kiosk".into(), acknowledged: false, expires_in_secs: Some(900), storage_ok: true },
        ),
        (
            "event.user_bootstrap.none",
            BridgeEvent::UserBootstrap { pending: false, username: "kiosk".into(), acknowledged: true, expires_in_secs: None, storage_ok: true },
        ),
        (
            "event.user_bootstrap_secret",
            BridgeEvent::UserBootstrapSecret { username: "kiosk".into(), password: secret(SYNTHETIC_PASSWORD), expires_in_secs: 885 },
        ),
        (
            "event.user_error",
            BridgeEvent::UserError {
                context: "user_bootstrap_ack".into(),
                code: "bad_ack".into(),
                message: "The last four characters do not match".into(),
            },
        ),
    ]
}

/// Expected `Debug` rendering of each parsed command fixture. `Secret`
/// renders as `<redacted>`, so the password itself is checked separately.
fn commands() -> Vec<(&'static str, &'static str)> {
    vec![
        ("command.bt_power_get", "BtPowerGet"),
        ("command.bt_set_power", "BtSetPower { powered: true }"),
        ("command.bt_scan_start", "BtScanStart"),
        ("command.bt_scan_stop", "BtScanStop"),
        ("command.bt_pair", r#"BtPair { address: "AA:BB:CC:DD:EE:FF" }"#),
        ("command.bt_pairing_confirm", "BtPairingConfirm { accept: true }"),
        ("command.bt_paired_list_get", "BtPairedListGet"),
        ("command.bt_connect", r#"BtConnect { address: "AA:BB:CC:DD:EE:FF" }"#),
        ("command.bt_disconnect", r#"BtDisconnect { address: "AA:BB:CC:DD:EE:FF" }"#),
        ("command.bt_forget", r#"BtForget { address: "AA:BB:CC:DD:EE:FF" }"#),
        ("command.brightness_get", "BrightnessGet"),
        ("command.brightness_set", "BrightnessSet { value: 80 }"),
        ("command.timezone_get", "TimezoneGet"),
        ("command.timezone_set", r#"TimezoneSet { value: "Europe/Lisbon" }"#),
        ("command.timezone_list_get", "TimezoneListGet"),
        ("command.wifi_get", "WifiGet"),
        ("command.wifi_scan", "WifiScan"),
        ("command.wifi_connect", r#"WifiConnect { ssid: "Home", password: <redacted> }"#),
        ("command.wifi_connect.open", r#"WifiConnect { ssid: "Cafe", password: <redacted> }"#),
        ("command.wifi_connect_saved", r#"WifiConnectSaved { ssid: "Home" }"#),
        ("command.wifi_disconnect", "WifiDisconnect"),
        ("command.wifi_saved_list_get", "WifiSavedListGet"),
        ("command.wifi_forget", r#"WifiForget { ssid: "Home" }"#),
        ("command.user_bootstrap_get", "UserBootstrapGet"),
        ("command.user_bootstrap_reveal", "UserBootstrapReveal"),
        ("command.user_bootstrap_ack", "UserBootstrapAck { last4: <redacted> }"),
    ]
}

#[test]
fn every_event_serializes_byte_for_byte_to_its_fixture() {
    for (name, event) in events() {
        let got = serde_json::to_string(&event).unwrap();
        assert_eq!(got, fixture(name), "fixture {name}");
    }
}

#[test]
fn every_command_fixture_parses_to_the_expected_variant() {
    for (name, expected) in commands() {
        let cmd: ClientCommand = serde_json::from_str(&fixture(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(format!("{cmd:?}"), expected, "fixture {name}");
    }
}

#[test]
fn wifi_connect_fixtures_carry_the_password_only_when_present() {
    let c: ClientCommand = serde_json::from_str(&fixture("command.wifi_connect")).unwrap();
    assert!(matches!(c, ClientCommand::WifiConnect { password, .. } if password.expose() == "hunter2hunter2"));
    let c: ClientCommand = serde_json::from_str(&fixture("command.wifi_connect.open")).unwrap();
    assert!(matches!(c, ClientCommand::WifiConnect { password, .. } if password.expose().is_empty()));
}

#[test]
fn user_bootstrap_ack_fixture_carries_the_last_four_characters() {
    let c: ClientCommand = serde_json::from_str(&fixture("command.user_bootstrap_ack")).unwrap();
    assert!(matches!(c, ClientCommand::UserBootstrapAck { last4 } if last4.expose() == "k7Qz"));
}

#[test]
fn only_the_secret_event_serializes_a_password() {
    for (name, event) in events() {
        let json = serde_json::to_string(&event).unwrap();
        let carries = json.contains(SYNTHETIC_PASSWORD);
        assert_eq!(carries, name == "event.user_bootstrap_secret", "fixture {name}");
    }
}

#[test]
fn no_event_leaks_a_password_through_debug_or_display() {
    for (name, event) in events() {
        let dbg = format!("{event:?}");
        assert!(!dbg.contains(SYNTHETIC_PASSWORD), "{name}: {dbg}");
    }
    let secret_event = events().into_iter().find(|(n, _)| *n == "event.user_bootstrap_secret").unwrap().1;
    assert!(format!("{secret_event:?}").contains("<redacted>"));
    let s = secret(SYNTHETIC_PASSWORD);
    assert!(!format!("{s}").contains(SYNTHETIC_PASSWORD));
    assert!(!format!("{s:?}").contains(SYNTHETIC_PASSWORD));
}

#[test]
fn fixture_directory_has_no_unreferenced_files() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../config/protocol/controller");
    let mut known: Vec<String> = events().into_iter().map(|(n, _)| format!("{n}.json")).collect();
    known.extend(commands().into_iter().map(|(n, _)| format!("{n}.json")));
    let mut on_disk: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.ends_with(".json"))
        .collect();
    known.sort();
    on_disk.sort();
    assert_eq!(on_disk, known);
}
