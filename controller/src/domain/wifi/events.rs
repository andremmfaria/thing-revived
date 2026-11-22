//! Parsing of unsolicited wpa_supplicant event lines.



// --- unsolicited events and connect progress ------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WpaEvent {
    Connected,
    Disconnected,
    ScanResults,
    ScanFailed,
    NetworkNotFound,
    AssocReject,
    AuthReject,
    /// `reason` is the raw `reason=` token (WRONG_KEY, AUTH_FAILED, CONN_FAILED, ...).
    TempDisabled { reason: String },
    /// "WPA: 4-Way Handshake failed - pre-shared key may be incorrect".
    HandshakeFailed,
    Other,
}

pub fn parse_event(line: &str) -> WpaEvent {
    // Event datagrams are prefixed with a priority like `<3>`.
    let line = match line.strip_prefix('<') {
        Some(rest) => rest.split_once('>').map_or(line, |(_, r)| r),
        None => line,
    };
    if line.starts_with("CTRL-EVENT-CONNECTED") {
        WpaEvent::Connected
    } else if line.starts_with("CTRL-EVENT-DISCONNECTED") {
        WpaEvent::Disconnected
    } else if line.starts_with("CTRL-EVENT-SCAN-RESULTS") {
        WpaEvent::ScanResults
    } else if line.starts_with("CTRL-EVENT-SCAN-FAILED") {
        WpaEvent::ScanFailed
    } else if line.starts_with("CTRL-EVENT-NETWORK-NOT-FOUND") {
        WpaEvent::NetworkNotFound
    } else if line.starts_with("CTRL-EVENT-ASSOC-REJECT") {
        WpaEvent::AssocReject
    } else if line.starts_with("CTRL-EVENT-AUTH-REJECT") {
        WpaEvent::AuthReject
    } else if line.starts_with("CTRL-EVENT-SSID-TEMP-DISABLED") {
        let reason = line
            .split_whitespace()
            .find_map(|t| t.strip_prefix("reason="))
            .unwrap_or("")
            .to_string();
        WpaEvent::TempDisabled { reason }
    } else if line.contains("4-Way Handshake failed") {
        WpaEvent::HandshakeFailed
    } else {
        WpaEvent::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_parsing() {
        assert_eq!(parse_event("<3>CTRL-EVENT-CONNECTED - Connection to aa:bb completed [id=0 id_str=]"), WpaEvent::Connected);
        assert_eq!(parse_event("<3>CTRL-EVENT-DISCONNECTED bssid=aa reason=3 locally_generated=1"), WpaEvent::Disconnected);
        assert_eq!(parse_event("<3>CTRL-EVENT-SCAN-RESULTS "), WpaEvent::ScanResults);
        assert_eq!(parse_event("<3>CTRL-EVENT-NETWORK-NOT-FOUND"), WpaEvent::NetworkNotFound);
        assert_eq!(
            parse_event("<3>CTRL-EVENT-SSID-TEMP-DISABLED id=0 ssid=\"Home\" auth_failures=1 duration=10 reason=WRONG_KEY"),
            WpaEvent::TempDisabled { reason: "WRONG_KEY".into() }
        );
        assert_eq!(parse_event("<3>WPA: 4-Way Handshake failed - pre-shared key may be incorrect"), WpaEvent::HandshakeFailed);
        assert_eq!(parse_event("CTRL-EVENT-ASSOC-REJECT bssid=aa status_code=17"), WpaEvent::AssocReject);
        assert_eq!(parse_event("<3>SME: Trying to authenticate"), WpaEvent::Other);
    }
}
