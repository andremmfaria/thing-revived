//! Pure parsing of `STATUS`/`SIGNAL_POLL` replies and the connection-state
//! derivation, plus the Ethernet-uplink check over plain interface rows.



/// Parsed `STATUS` reply (key=value lines); only the keys this UI uses.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WpaStatus {
    pub wpa_state: String,
    pub ssid: Option<String>,
    pub ip_address: Option<String>,
}

pub fn parse_status(reply: &str) -> WpaStatus {
    let mut st = WpaStatus::default();
    for line in reply.lines() {
        if let Some((k, v)) = line.split_once('=') {
            match k {
                "wpa_state" => st.wpa_state = v.to_string(),
                // `ssid=` also appears under p2p keys with other prefixes, so
                // only the exact key counts.
                "ssid" => st.ssid = Some(v.to_string()),
                "ip_address" => st.ip_address = Some(v.to_string()),
                _ => {}
            }
        }
    }
    st
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnState {
    Connected,
    ObtainingIp,
    Connecting,
    Disconnected,
}

impl ConnState {
    pub fn wire(self) -> &'static str {
        match self {
            ConnState::Connected => "connected",
            ConnState::ObtainingIp => "obtaining_ip",
            ConnState::Connecting => "connecting",
            ConnState::Disconnected => "disconnected",
        }
    }
}

pub fn derive_state(st: &WpaStatus) -> ConnState {
    match st.wpa_state.as_str() {
        "COMPLETED" => {
            if st.ip_address.as_deref().is_some_and(|ip| !ip.is_empty()) {
                ConnState::Connected
            } else {
                ConnState::ObtainingIp
            }
        }
        "AUTHENTICATING" | "ASSOCIATING" | "ASSOCIATED" | "4WAY_HANDSHAKE" | "GROUP_HANDSHAKE" => {
            ConnState::Connecting
        }
        _ => ConnState::Disconnected,
    }
}

/// Extracts `RSSI=` from a `SIGNAL_POLL` reply.
pub fn parse_rssi(reply: &str) -> Option<i32> {
    reply.lines().find_map(|l| l.strip_prefix("RSSI=")?.trim().parse().ok())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusInfo {
    pub state: ConnState,
    pub ssid: Option<String>,
    pub ip: Option<String>,
    pub interface: String,
    pub signal_pct: Option<u8>,
    pub ethernet_up: bool,
}

/// One row of `/sys/class/net`, reduced to what the Ethernet check needs.
#[derive(Clone, Debug)]
pub struct NetIf {
    pub name: String,
    /// ARPHRD type (`/sys/class/net/<if>/type`); 1 is Ethernet.
    pub if_type: u32,
    pub operstate: String,
    pub wireless: bool,
}

/// True when any wired Ethernet-type interface is up. The USB gadget
/// interface (`uncm*`) and virtual links are excluded: `uncm0` is the
/// CDC-NCM SSH link, not uplink connectivity.
pub fn ethernet_is_up(ifs: &[NetIf]) -> bool {
    ifs.iter().any(|i| {
        i.if_type == 1
            && !i.wireless
            && i.operstate == "up"
            && !["lo", "uncm", "veth", "docker", "br-", "usb"].iter().any(|p| i.name.starts_with(p))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_derivation() {
        let connected = parse_status("bssid=aa\nfreq=2412\nssid=Home\nid=0\nwpa_state=COMPLETED\nip_address=192.168.1.20\n");
        assert_eq!(connected.ssid.as_deref(), Some("Home"));
        assert_eq!(derive_state(&connected), ConnState::Connected);

        let no_ip = parse_status("ssid=Home\nwpa_state=COMPLETED\n");
        assert_eq!(derive_state(&no_ip), ConnState::ObtainingIp);

        for s in ["ASSOCIATING", "AUTHENTICATING", "4WAY_HANDSHAKE", "GROUP_HANDSHAKE", "ASSOCIATED"] {
            assert_eq!(derive_state(&parse_status(&format!("wpa_state={s}\n"))), ConnState::Connecting, "{s}");
        }
        for s in ["DISCONNECTED", "INACTIVE", "SCANNING", "INTERFACE_DISABLED", ""] {
            assert_eq!(derive_state(&parse_status(&format!("wpa_state={s}\n"))), ConnState::Disconnected, "{s}");
        }
    }

    #[test]
    fn rssi_parse() {
        assert_eq!(parse_rssi("RSSI=-54\nLINKSPEED=72\nNOISE=9999\nFREQUENCY=2412\n"), Some(-54));
        assert_eq!(parse_rssi("FAIL\n"), None);
    }

    #[test]
    fn ethernet_detection_ignores_gadget_wifi_and_down_links() {
        let mk = |name: &str, t: u32, op: &str, w: bool| NetIf { name: name.into(), if_type: t, operstate: op.into(), wireless: w };
        assert!(ethernet_is_up(&[mk("eth0", 1, "up", false)]));
        assert!(!ethernet_is_up(&[mk("eth0", 1, "down", false)]));
        assert!(!ethernet_is_up(&[mk("uncm0", 1, "up", false)]));
        assert!(!ethernet_is_up(&[mk("wlan0", 1, "up", true)]));
        assert!(!ethernet_is_up(&[mk("lo", 772, "unknown", false)]));
        assert!(ethernet_is_up(&[mk("lo", 772, "unknown", false), mk("eth0", 1, "up", false)]));
    }
}
