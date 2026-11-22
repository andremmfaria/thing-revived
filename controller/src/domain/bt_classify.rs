//! Pure Bluetooth classification and address/path helpers (no D-Bus).

use crate::domain::bluetooth::ADAPTER_PATH;

pub fn device_path_to_address(path: &str) -> String {
    // "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF" -> "AA:BB:CC:DD:EE:FF"
    path.rsplit('/')
        .next()
        .unwrap_or("")
        .trim_start_matches("dev_")
        .replace('_', ":")
}

pub fn address_to_device_path(address: &str) -> String {
    format!("{ADAPTER_PATH}/dev_{}", address.replace(':', "_"))
}


/// Signal 1's pure classification logic, split out from `device_is_hid_keyboard`
/// so it's unit-testable without a live D-Bus connection: classic
/// Class-of-Device, major=Peripheral (0x05, mask 0x1F00), minor bits 7-6
/// (mask 0xC0) = Keyboard (0x40)/Pointing (0x80)/Combo (0xC0).
pub fn class_of_device_is_keyboard(class: u32) -> bool {
    (class & 0x1F00) == 0x0500 && (class & 0xC0) != 0
}

/// Signal 2's pure classification logic: BlueZ's standard freedesktop.org
/// icon name for keyboard-class devices.
pub fn icon_is_keyboard(icon: &str) -> bool {
    icon == "input-keyboard"
}

/// Signal 3's pure classification logic: advertised HID service UUIDs
/// (classic HID Profile 0x1124, BLE HOGP 0x1812) - match on the short-form
/// prefix of the full 128-bit UUID strings BlueZ reports.
pub fn uuids_contain_hid_service(uuids: &[String]) -> bool {
    uuids.iter().any(|u| {
        let u = u.to_lowercase();
        u.starts_with("00001124-") || u.starts_with("00001812-")
    })
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_path_round_trips_to_colon_address() {
        assert_eq!(
            device_path_to_address("/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF"),
            "AA:BB:CC:DD:EE:FF"
        );
    }

    #[test]
    fn address_round_trips_to_device_path() {
        assert_eq!(
            address_to_device_path("AA:BB:CC:DD:EE:FF"),
            "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF"
        );
    }

    #[test]
    fn address_and_path_conversions_are_inverses() {
        let addr = "01:23:45:67:89:AB";
        assert_eq!(device_path_to_address(&address_to_device_path(addr)), addr);
    }

    // §13.4's example CoD values, cross-checked against the Bluetooth SIG
    // assigned-numbers baseband encoding (major class bits 12-8, minor class
    // bits 7-2): 0x002540 = major Peripheral (0x05) + minor Keyboard (0x40),
    // a real, commonly-cited example keyboard Class-of-Device value (e.g.
    // the one used in BT-Manager's own cod.py worked examples).
    #[test]
    fn class_of_device_classifies_known_keyboard_value() {
        assert!(class_of_device_is_keyboard(0x002540));
    }

    #[test]
    fn class_of_device_classifies_known_combo_keyboard_pointing_value() {
        // Major Peripheral (0x05) + minor Combo keyboard/pointing (0xC0).
        assert!(class_of_device_is_keyboard(0x0025C0));
    }

    #[test]
    fn class_of_device_rejects_known_phone_value() {
        // 0x5A020C: major Phone (0x02, bits 12-8), not Peripheral - a
        // standard smartphone Class-of-Device value (SIG major class 0x02 =
        // "Phone"), must not be classified as a keyboard.
        assert!(!class_of_device_is_keyboard(0x5A020C));
    }

    #[test]
    fn class_of_device_rejects_peripheral_with_no_keyboard_pointing_minor_bits() {
        // Major Peripheral (0x05) but minor bits 7-6 both zero (e.g. a
        // joystick/gamepad, minor class 0x01 in the Peripheral table) -
        // Peripheral-major alone is not sufficient, the 0xC0 minor-bit mask
        // must also be checked.
        assert!(!class_of_device_is_keyboard(0x050104));
    }

    #[test]
    fn icon_matches_only_the_exact_bluez_keyboard_icon_name() {
        assert!(icon_is_keyboard("input-keyboard"));
        assert!(!icon_is_keyboard("audio-card"));
        assert!(!icon_is_keyboard("phone"));
    }

    #[test]
    fn uuids_detect_classic_hid_profile_uuid() {
        let uuids = vec!["00001124-0000-1000-8000-00805f9b34fb".to_string()];
        assert!(uuids_contain_hid_service(&uuids));
    }

    #[test]
    fn uuids_detect_ble_hogp_uuid_case_insensitively() {
        let uuids = vec!["00001812-0000-1000-8000-00805F9B34FB".to_string()];
        assert!(uuids_contain_hid_service(&uuids));
    }

    #[test]
    fn uuids_reject_unrelated_service_uuids() {
        // A1D (Audio/Video Remote Control, 0x110E) - a real BlueZ-advertised
        // UUID that is not an HID service, confirming the match isn't
        // accidentally over-broad.
        let uuids = vec!["0000110e-0000-1000-8000-00805f9b34fb".to_string()];
        assert!(!uuids_contain_hid_service(&uuids));
    }
}
