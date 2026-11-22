//! Input validation and wpa_supplicant argument encoding.

use super::scan::Security;

// --- input validation and wpa_supplicant argument encoding ---------------

pub fn validate_ssid(ssid: &str) -> Result<(), String> {
    if ssid.is_empty() {
        return Err("SSID is empty".into());
    }
    if ssid.len() > 32 {
        return Err("SSID is longer than 32 bytes".into());
    }
    if ssid.chars().any(|c| c.is_control()) {
        return Err("SSID contains control characters".into());
    }
    Ok(())
}

/// WPA passphrases are 8..=63 printable ASCII characters. The ASCII limit
/// also keeps the quoted `psk "..."` argument free of newlines, which would
/// split the ctrl command. Open networks must have no password.
pub fn validate_password(password: &str, security: Security) -> Result<(), String> {
    if !security.supported() {
        return Err(format!("{} networks are not supported", security.wire()));
    }
    if !security.needs_password() {
        return if password.is_empty() { Ok(()) } else { Err("open network takes no password".into()) };
    }
    let n = password.chars().count();
    if !(8..=63).contains(&n) {
        return Err("Password must be 8 to 63 characters".into());
    }
    if !password.chars().all(|c| (' '..='~').contains(&c)) {
        return Err("Password may only contain printable ASCII characters".into());
    }
    Ok(())
}

pub fn hex_encode(s: &str) -> String {
    s.bytes().map(|b| format!("{b:02x}")).collect()
}

/// `SET_NETWORK <id> psk "<password>"`. wpa_supplicant takes the text between
/// the first and the LAST double quote (os_strrchr), without backslash
/// processing, so an embedded `"` or `\` needs no escaping once the value is
/// validated printable ASCII. SSIDs are sent hex-encoded instead (no quoting
/// question at all). Never log the returned string.
pub fn set_psk_command(id: u32, password: &str) -> String {
    format!("SET_NETWORK {id} psk \"{password}\"")
}

/// The `SET_NETWORK` argument lists (without the id) for a new network.
/// The psk is returned separately from the rest so callers can keep it out
/// of anything that gets logged.
pub fn network_settings(ssid: &str, security: Security) -> Vec<(&'static str, String)> {
    let mut v = vec![("ssid", hex_encode(ssid))];
    match security {
        Security::Open => v.push(("key_mgmt", "NONE".into())),
        Security::Psk => v.push(("key_mgmt", "WPA-PSK".into())),
        Security::Sae => {
            v.push(("key_mgmt", "SAE".into()));
            v.push(("ieee80211w", "2".into()));
        }
        Security::PskSae => {
            v.push(("key_mgmt", "WPA-PSK SAE".into()));
            v.push(("ieee80211w", "1".into()));
        }
        Security::Wep | Security::Eap => {}
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssid_validation() {
        assert!(validate_ssid("Home").is_ok());
        assert!(validate_ssid(&"x".repeat(32)).is_ok());
        assert!(validate_ssid("").is_err());
        assert!(validate_ssid(&"x".repeat(33)).is_err());
        assert!(validate_ssid("a\nb").is_err());
        // Length is in bytes, not chars: 17 two-byte chars is 34 bytes.
        assert!(validate_ssid(&"é".repeat(17)).is_err());
    }

    #[test]
    fn password_validation() {
        assert!(validate_password("12345678", Security::Psk).is_ok());
        assert!(validate_password(&"a".repeat(63), Security::Sae).is_ok());
        assert!(validate_password("1234567", Security::Psk).is_err());
        assert!(validate_password(&"a".repeat(64), Security::Psk).is_err());
        assert!(validate_password("pass word with \"quotes\" and \\", Security::PskSae).is_ok());
        assert!(validate_password("line\nbreak12", Security::Psk).is_err());
        assert!(validate_password("caf\u{e9}-password", Security::Psk).is_err());
        assert!(validate_password("", Security::Open).is_ok());
        assert!(validate_password("x", Security::Open).is_err());
        assert!(validate_password("12345678", Security::Wep).is_err());
        assert!(validate_password("12345678", Security::Eap).is_err());
    }

    #[test]
    fn ssid_is_hex_encoded_and_psk_quoted() {
        assert_eq!(hex_encode("Ab 1"), "41622031");
        assert_eq!(hex_encode("a\"b"), "612262");
        assert_eq!(set_psk_command(3, "pa\"ss\\word"), "SET_NETWORK 3 psk \"pa\"ss\\word\"");
    }

    #[test]
    fn network_settings_per_security() {
        let get = |s: Security| network_settings("Hi", s);
        assert_eq!(get(Security::Open), vec![("ssid", "4869".to_string()), ("key_mgmt", "NONE".to_string())]);
        assert!(get(Security::Psk).contains(&("key_mgmt", "WPA-PSK".to_string())));
        let sae = get(Security::Sae);
        assert!(sae.contains(&("key_mgmt", "SAE".to_string())) && sae.contains(&("ieee80211w", "2".to_string())));
        let mix = get(Security::PskSae);
        assert!(mix.contains(&("key_mgmt", "WPA-PSK SAE".to_string())) && mix.contains(&("ieee80211w", "1".to_string())));
        assert!(!get(Security::Psk).iter().any(|(k, _)| *k == "psk"));
    }
}
