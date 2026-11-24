//! Golden wire test: every `BridgeEvent` serializes byte-for-byte to its
//! committed fixture and every `ClientCommand` fixture parses to the
//! expected variant. Fixtures live in config/protocol/controller/ and are shared
//! with the frontend tests (see config/protocol/controller/README.md).

use controller_agent::protocol::{
    BootstrapPhaseWire, BridgeEvent, ClientCommand, GroupWire, PairedDeviceWire, Secret, SshKeyWire, UserWire, WifiNetworkWire, WifiSavedWire,
    DEBUG_ERROR_CODES, USB_ROLE_ERROR_CODES, USER_ERROR_CODES,
};
use std::path::PathBuf;

fn fixture(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../config/protocol/controller").join(format!("{name}.json"));
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    // Exactly one trailing newline is part of the file convention, not of the message.
    raw.strip_suffix('\n').unwrap_or_else(|| panic!("{} must end with a newline", path.display())).to_string()
}

/// Synthetic first-boot password used by the fixtures and the redaction tests.
const SYNTHETIC_PASSWORD: &str = "Xe4mH7pQ9tLwZc3K";

/// Events allowed to serialize a password.
const SECRET_EVENTS: &[&str] = &["event.user_bootstrap_secret", "event.user_secret"];

fn secret(value: &str) -> Secret {
    serde_json::from_str(&serde_json::to_string(value).unwrap()).unwrap()
}

fn bootstrap(phase: BootstrapPhaseWire, storage_ok: bool) -> BridgeEvent {
    BridgeEvent::UserBootstrap { phase, username: "kiosk".into(), storage_ok }
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
        ("event.usb_role", BridgeEvent::UsbRole { role: Some("device".into()), boot_role: Some("host".into()), adb_forced: false }),
        ("event.usb_role.adb_forced", BridgeEvent::UsbRole { role: Some("device".into()), boot_role: None, adb_forced: true }),
        (
            "event.usb_role_error",
            BridgeEvent::UsbRoleError { context: "usb_role_set".into(), code: "invalid_role".into(), message: "'Host' is not a USB role (device or host)".into() },
        ),
        ("event.usb_role_rebooting", BridgeEvent::UsbRoleRebooting {}),
        (
            "event.debug_state",
            BridgeEvent::DebugState {
                adb: false,
                uart: false,
                adb_build_default: false,
                uart_build_default: false,
                adb_available: true,
                blocked: None,
            },
        ),
        (
            "event.debug_state.enabled",
            BridgeEvent::DebugState {
                adb: true,
                uart: true,
                adb_build_default: false,
                uart_build_default: true,
                adb_available: true,
                blocked: None,
            },
        ),
        (
            "event.debug_state.blocked_host",
            BridgeEvent::DebugState {
                adb: false,
                uart: false,
                adb_build_default: false,
                uart_build_default: false,
                adb_available: false,
                blocked: Some("usb_role_host".into()),
            },
        ),
        (
            "event.debug_error",
            BridgeEvent::DebugError {
                context: "debug_set".into(),
                code: "not_unlocked".into(),
                message: "Sign in as an admin to change debug access".into(),
            },
        ),
        (
            "event.debug_error.confirm_required",
            BridgeEvent::DebugError {
                context: "debug_set".into(),
                code: "confirm_required".into(),
                message: "Hold the toggle to confirm".into(),
            },
        ),
        (
            "event.debug_error.usb_role_host",
            BridgeEvent::DebugError {
                context: "debug_set".into(),
                code: "usb_role_host".into(),
                message: "Switch the USB port to device mode first".into(),
            },
        ),
        (
            "event.debug_error.busy",
            BridgeEvent::DebugError {
                context: "debug_set".into(),
                code: "busy".into(),
                message: "Another debug access change is already in progress".into(),
            },
        ),
        (
            "event.debug_error.unavailable",
            BridgeEvent::DebugError {
                context: "debug_access".into(),
                code: "unavailable".into(),
                message: "Debug access control is not available".into(),
            },
        ),
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
        ("event.user_bootstrap.none", bootstrap(BootstrapPhaseWire::None, true)),
        ("event.user_bootstrap.prompt", bootstrap(BootstrapPhaseWire::Prompt, true)),
        ("event.user_bootstrap.generating", bootstrap(BootstrapPhaseWire::Generating, true)),
        ("event.user_bootstrap.showing", bootstrap(BootstrapPhaseWire::Showing, true)),
        ("event.user_bootstrap.done", bootstrap(BootstrapPhaseWire::Done, true)),
        ("event.user_bootstrap.storage_failed", bootstrap(BootstrapPhaseWire::None, false)),
        ("event.user_bootstrap_secret", BridgeEvent::UserBootstrapSecret { username: "kiosk".into(), password: secret(SYNTHETIC_PASSWORD) }),
        (
            "event.user_error",
            BridgeEvent::UserError {
                context: "user_bootstrap_get".into(),
                code: "generate_failed".into(),
                message: "Could not set the password".into(),
            },
        ),
        ("event.user_session", BridgeEvent::UserSession { unlocked: true, expires_in_secs: Some(300), username: Some("ana".into()) }),
        ("event.user_session.locked", BridgeEvent::UserSession { unlocked: false, expires_in_secs: None, username: None }),
        (
            "event.user_list",
            BridgeEvent::UserList {
                users: vec![
                    UserWire {
                        name: "kiosk".into(),
                        uid: 1000,
                        gid: 1000,
                        home: "/var/home/kiosk".into(),
                        shell: "/bin/bash".into(),
                        locked: false,
                        admin: true,
                        system: false,
                        protected: true,
                        groups: vec!["sudo".into(), "adm".into()],
                        key_count: 0,
                    },
                    UserWire {
                        name: "ana".into(),
                        uid: 1001,
                        gid: 1001,
                        home: "/var/home/ana".into(),
                        shell: "/bin/bash".into(),
                        locked: true,
                        admin: false,
                        system: false,
                        protected: false,
                        groups: vec![],
                        key_count: 2,
                    },
                ],
            },
        ),
        (
            "event.user_result",
            BridgeEvent::UserResult { op: "user_add".into(), name: "ana".into(), success: true, error: None, message: None },
        ),
        (
            "event.user_result.failed",
            BridgeEvent::UserResult {
                op: "user_remove".into(),
                name: "kiosk".into(),
                success: false,
                error: Some("protected_account".into()),
                message: Some("The kiosk account cannot be removed".into()),
            },
        ),
        ("event.user_secret", BridgeEvent::UserSecret { name: "ana".into(), password: secret(SYNTHETIC_PASSWORD) }),
        (
            "event.group_list",
            BridgeEvent::GroupList {
                groups: vec![
                    GroupWire { name: "sudo".into(), gid: 27, system: true, assignable: true, members: vec!["kiosk".into()] },
                    GroupWire { name: "team".into(), gid: 1002, system: false, assignable: true, members: vec![] },
                ],
            },
        ),
        (
            "event.ssh_key_list",
            BridgeEvent::SshKeyList {
                name: "ana".into(),
                keys: vec![SshKeyWire {
                    fingerprint: "SHA256:abc123def456ghi789jkl012mno345pqr678stu901v".into(),
                    key_type: "ssh-ed25519".into(),
                    comment: "ana@laptop".into(),
                }],
            },
        ),
        ("event.ssh_key_list.empty", BridgeEvent::SshKeyList { name: "kiosk".into(), keys: vec![] }),
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
        ("command.usb_role_get", "UsbRoleGet"),
        ("command.usb_role_set", r#"UsbRoleSet { role: "host" }"#),
        ("command.usb_role_reboot", "UsbRoleReboot"),
        ("command.debug_get", "DebugGet"),
        ("command.debug_set", r#"DebugSet { surface: "adb", enabled: true, confirm: "adb" }"#),
        ("command.debug_set.disable", r#"DebugSet { surface: "uart", enabled: false, confirm: "uart" }"#),
        ("command.wifi_get", "WifiGet"),
        ("command.wifi_scan", "WifiScan"),
        ("command.wifi_connect", r#"WifiConnect { ssid: "Home", password: <redacted> }"#),
        ("command.wifi_connect.open", r#"WifiConnect { ssid: "Cafe", password: <redacted> }"#),
        ("command.wifi_connect_saved", r#"WifiConnectSaved { ssid: "Home" }"#),
        ("command.wifi_disconnect", "WifiDisconnect"),
        ("command.wifi_saved_list_get", "WifiSavedListGet"),
        ("command.wifi_forget", r#"WifiForget { ssid: "Home" }"#),
        ("command.user_bootstrap_get", "UserBootstrapGet"),
        ("command.user_unlock", r#"UserUnlock { username: "ana", password: <redacted> }"#),
        ("command.user_lock_session", "UserLockSession"),
        ("command.user_list_get", "UserListGet"),
        ("command.user_add", r#"UserAdd { name: "ana", password: <redacted>, admin: false }"#),
        ("command.user_add.generate", r#"UserAdd { name: "ana", password: <redacted>, admin: true }"#),
        ("command.user_remove", r#"UserRemove { name: "ana", keep_home: false, confirm: "ana" }"#),
        ("command.user_set_locked", r#"UserSetLocked { name: "ana", locked: true, confirm: "ana" }"#),
        ("command.user_set_password", r#"UserSetPassword { name: "ana", password: <redacted>, current_password: <redacted>, confirm: "" }"#),
        ("command.user_set_password.generate", r#"UserSetPassword { name: "ana", password: <redacted>, current_password: <redacted>, confirm: "ana" }"#),
        ("command.group_list_get", "GroupListGet"),
        ("command.user_group_add", r#"UserGroupAdd { name: "ana", group: "sudo", confirm: "sudo" }"#),
        ("command.user_group_remove", r#"UserGroupRemove { name: "ana", group: "adm" }"#),
        ("command.group_add", r#"GroupAdd { name: "team" }"#),
        ("command.group_remove", r#"GroupRemove { name: "team", confirm: "team" }"#),
        ("command.ssh_key_list_get", r#"SshKeyListGet { name: "ana" }"#),
        ("command.ssh_key_add", r#"SshKeyAdd { name: "ana", key: "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIExampleSyntheticKeyMaterial000000000 ana@laptop" }"#),
        ("command.ssh_key_remove", r#"SshKeyRemove { name: "ana", fingerprint: "SHA256:abc123def456ghi789jkl012mno345pqr678stu901v", confirm: "SHA256:abc123def456ghi789jkl012mno345pqr678stu901v" }"#),
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
fn user_command_fixtures_carry_their_passwords_and_generate_variants_are_empty() {
    let pw = |name: &str| match serde_json::from_str::<ClientCommand>(&fixture(name)).unwrap() {
        ClientCommand::UserAdd { password, .. } | ClientCommand::UserSetPassword { password, .. } => password,
        other => panic!("{name}: {other:?}"),
    };
    assert!(!pw("command.user_add").expose().is_empty());
    assert!(pw("command.user_add.generate").expose().is_empty());
    assert!(!pw("command.user_set_password").expose().is_empty());
    assert!(pw("command.user_set_password.generate").expose().is_empty());
}

#[test]
fn the_removed_bootstrap_commands_and_error_codes_are_gone() {
    for t in ["user_bootstrap_reveal", "user_bootstrap_ack"] {
        let raw = format!(r#"{{"type":"{t}"}}"#);
        assert!(serde_json::from_str::<ClientCommand>(&raw).is_err(), "{t}");
    }
    for code in ["bad_ack", "press_required", "expired", "not_pending"] {
        assert!(!USER_ERROR_CODES.contains(&code), "{code}");
    }
    assert!(USER_ERROR_CODES.contains(&"generate_failed") && USER_ERROR_CODES.contains(&"ack_failed"));
}

#[test]
fn bootstrap_events_carry_no_pending_expiry_or_baked_fields() {
    for (name, event) in events() {
        let json = serde_json::to_string(&event).unwrap();
        for stale in ["expires_in_secs\":900", "\"pending\"", "\"acknowledged\"", "\"baked\"", "last4"] {
            assert!(!json.contains(stale), "{name}: {stale}");
        }
    }
}

#[test]
fn only_the_secret_event_serializes_a_password() {
    for (name, event) in events() {
        let json = serde_json::to_string(&event).unwrap();
        let carries = json.contains(SYNTHETIC_PASSWORD);
        assert_eq!(carries, SECRET_EVENTS.contains(&name), "fixture {name}");
    }
}

#[test]
fn no_event_leaks_a_password_through_debug_or_display() {
    for (name, event) in events() {
        let dbg = format!("{event:?}");
        assert!(!dbg.contains(SYNTHETIC_PASSWORD), "{name}: {dbg}");
    }
    for secret_name in SECRET_EVENTS {
        let secret_event = events().into_iter().find(|(n, _)| n == secret_name).unwrap().1;
        assert!(format!("{secret_event:?}").contains("<redacted>"), "{secret_name}");
    }
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

#[test]
fn usb_role_error_codes_cover_what_the_service_emits() {
    assert_eq!(USB_ROLE_ERROR_CODES, &["invalid_role", "unavailable", "failed"]);
    let e: BridgeEvent = events().into_iter().find(|(n, _)| *n == "event.usb_role_error").unwrap().1;
    if let BridgeEvent::UsbRoleError { code, .. } = e {
        assert!(USB_ROLE_ERROR_CODES.contains(&code.as_str()));
    } else {
        panic!("not a usb_role_error");
    }
}

#[test]
fn usb_role_set_with_a_missing_role_does_not_parse() {
    assert!(serde_json::from_str::<ClientCommand>(r#"{"type":"usb_role_set"}"#).is_err());
}

#[test]
fn debug_error_codes_cover_what_the_service_emits() {
    assert_eq!(
        DEBUG_ERROR_CODES,
        &["not_unlocked", "confirm_required", "invalid_surface", "usb_role_host", "toggle_failed", "busy", "unavailable", "rate_limited"]
    );
    for name in [
        "event.debug_error",
        "event.debug_error.confirm_required",
        "event.debug_error.usb_role_host",
        "event.debug_error.busy",
        "event.debug_error.unavailable",
    ] {
        let e = events().into_iter().find(|(n, _)| *n == name).unwrap().1;
        if let BridgeEvent::DebugError { code, .. } = e {
            assert!(DEBUG_ERROR_CODES.contains(&code.as_str()), "{name}: {code}");
        } else {
            panic!("{name} is not a debug_error");
        }
    }
}

#[test]
fn debug_set_with_a_missing_surface_does_not_parse() {
    assert!(serde_json::from_str::<ClientCommand>(r#"{"type":"debug_set","enabled":true}"#).is_err());
}
