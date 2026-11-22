//! Pure parsing of `SCAN_RESULTS` and `LIST_NETWORKS` replies, plus the
//! security classification and signal mapping.

use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Security {
    Open,
    /// WPA/WPA2 pre-shared key.
    Psk,
    /// WPA3 SAE only.
    Sae,
    /// WPA2-PSK and WPA3-SAE transition mode.
    PskSae,
    /// Listed but not connectable from this UI.
    Wep,
    /// 802.1X / enterprise, listed but not connectable from this UI.
    Eap,
}

impl Security {
    pub fn wire(self) -> &'static str {
        match self {
            Security::Open => "open",
            Security::Psk => "wpa2",
            Security::Sae => "wpa3",
            Security::PskSae => "wpa2/wpa3",
            Security::Wep => "wep",
            Security::Eap => "eap",
        }
    }

    pub fn supported(self) -> bool {
        !matches!(self, Security::Wep | Security::Eap)
    }

    pub fn needs_password(self) -> bool {
        !matches!(self, Security::Open)
    }
}

/// Derives security from a wpa_supplicant scan-result flags string such as
/// `[WPA2-PSK-CCMP][ESS]` or `[RSN-SAE-CCMP][ESS]`.
pub fn security_from_flags(flags: &str) -> Security {
    let has = |s: &str| flags.contains(s);
    if has("-EAP") {
        return Security::Eap;
    }
    let sae = has("-SAE");
    let psk = has("-PSK");
    match (psk, sae) {
        (true, true) => Security::PskSae,
        (false, true) => Security::Sae,
        (true, false) => Security::Psk,
        (false, false) => {
            if has("WEP") {
                Security::Wep
            } else {
                Security::Open
            }
        }
    }
}

/// Maps RSSI in dBm to 0..=100: -50 dBm or better is 100, -100 dBm or worse is 0.
pub fn dbm_to_pct(dbm: i32) -> u8 {
    (2 * (dbm + 100)).clamp(0, 100) as u8
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetworkInfo {
    pub ssid: String,
    pub signal_dbm: i32,
    pub signal_pct: u8,
    pub security: Security,
}

/// Parses `SCAN_RESULTS` (tab separated: bssid, frequency, signal level,
/// flags, ssid). Hidden (empty SSID) entries are dropped since hidden SSIDs
/// are out of scope. Duplicate SSIDs (several APs, 2.4 and 5 GHz) collapse to
/// the strongest one. Result is sorted strongest first, ties by SSID.
///
/// wpa_supplicant truncates this reply at 4096 bytes (`reply_size` in
/// ctrl_iface.c), so an unterminated last line is discarded rather than
/// shown with a cut-off SSID.
pub fn parse_scan_results(reply: &str) -> Vec<NetworkInfo> {
    let mut lines: Vec<&str> = reply.split('\n').collect();
    // `split` leaves the text after the final '\n' last; if the reply was
    // cut mid-line that remainder is non-empty and incomplete.
    if lines.last().is_some() {
        lines.pop();
    }
    let mut best: HashMap<String, NetworkInfo> = HashMap::new();
    for line in lines.iter().skip(1) {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() < 5 {
            continue;
        }
        let ssid = f[4].trim_end_matches('\r');
        if ssid.is_empty() {
            continue;
        }
        let dbm: i32 = match f[2].trim().parse() {
            Ok(v) => v,
            Err(_) => continue,
        };
        let info = NetworkInfo {
            ssid: ssid.to_string(),
            signal_dbm: dbm,
            signal_pct: dbm_to_pct(dbm),
            security: security_from_flags(f[3]),
        };
        match best.get(ssid) {
            Some(existing) if existing.signal_dbm >= dbm => {}
            _ => {
                best.insert(ssid.to_string(), info);
            }
        }
    }
    let mut out: Vec<NetworkInfo> = best.into_values().collect();
    out.sort_by(|a, b| b.signal_dbm.cmp(&a.signal_dbm).then_with(|| a.ssid.cmp(&b.ssid)));
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedNetwork {
    pub id: u32,
    pub ssid: String,
    pub current: bool,
}

/// Parses `LIST_NETWORKS` (tab separated: id, ssid, bssid, flags).
pub fn parse_list_networks(reply: &str) -> Vec<SavedNetwork> {
    reply
        .lines()
        .skip(1)
        .filter_map(|line| {
            let f: Vec<&str> = line.split('\t').collect();
            if f.len() < 2 {
                return None;
            }
            let id: u32 = f[0].trim().parse().ok()?;
            Some(SavedNetwork {
                id,
                ssid: f[1].to_string(),
                current: f.get(3).is_some_and(|flags| flags.contains("[CURRENT]")),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCAN: &str = "bssid / frequency / signal level / flags / ssid\n\
aa:bb:cc:00:00:01\t2412\t-70\t[WPA2-PSK-CCMP][ESS]\tHome\n\
aa:bb:cc:00:00:02\t5180\t-52\t[WPA2-PSK-CCMP][ESS]\tHome\n\
aa:bb:cc:00:00:03\t2437\t-60\t[RSN-SAE-CCMP][ESS]\tSecure3\n\
aa:bb:cc:00:00:04\t2437\t-85\t[ESS]\tCafe Free\n\
aa:bb:cc:00:00:05\t2437\t-40\t[WPA2-PSK-CCMP][ESS]\t\n\
aa:bb:cc:00:00:06\t2462\t-90\t[WEP][ESS]\tOld\n\
aa:bb:cc:00:00:07\t2462\t-65\t[WPA2-EAP-CCMP][ESS]\tCorp\n\
aa:bb:cc:00:00:08\t2462\t-80\t[WPA2-PSK-CCMP][RSN-SAE-CCMP][ESS]\tMixed\n";

    #[test]
    fn scan_dedups_by_ssid_keeping_strongest_and_sorts() {
        let nets = parse_scan_results(SCAN);
        let names: Vec<&str> = nets.iter().map(|n| n.ssid.as_str()).collect();
        assert_eq!(names, vec!["Home", "Secure3", "Corp", "Mixed", "Cafe Free", "Old"]);
        let home = &nets[0];
        assert_eq!(home.signal_dbm, -52);
        assert_eq!(home.signal_pct, 96);
    }

    #[test]
    fn scan_drops_hidden_ssids() {
        assert!(!parse_scan_results(SCAN).iter().any(|n| n.ssid.is_empty()));
    }

    #[test]
    fn scan_security_classification() {
        let nets = parse_scan_results(SCAN);
        let sec = |s: &str| nets.iter().find(|n| n.ssid == s).unwrap().security;
        assert_eq!(sec("Home"), Security::Psk);
        assert_eq!(sec("Secure3"), Security::Sae);
        assert_eq!(sec("Cafe Free"), Security::Open);
        assert_eq!(sec("Old"), Security::Wep);
        assert_eq!(sec("Corp"), Security::Eap);
        assert_eq!(sec("Mixed"), Security::PskSae);
    }

    #[test]
    fn scan_discards_unterminated_last_line() {
        let cut = "bssid / frequency / signal level / flags / ssid\n\
aa:bb:cc:00:00:01\t2412\t-70\t[WPA2-PSK-CCMP][ESS]\tHome\n\
aa:bb:cc:00:00:02\t2412\t-50\t[WPA2-PSK-CCMP][ESS]\tTruncat";
        let nets = parse_scan_results(cut);
        assert_eq!(nets.len(), 1);
        assert_eq!(nets[0].ssid, "Home");
    }

    #[test]
    fn scan_tolerates_garbage_lines() {
        let nets = parse_scan_results("header\nnot a row\nx\t1\tNaN\t[ESS]\tbad\n");
        assert!(nets.is_empty());
        assert!(parse_scan_results("").is_empty());
    }

    #[test]
    fn dbm_mapping_is_clamped() {
        assert_eq!(dbm_to_pct(-30), 100);
        assert_eq!(dbm_to_pct(-50), 100);
        assert_eq!(dbm_to_pct(-75), 50);
        assert_eq!(dbm_to_pct(-100), 0);
        assert_eq!(dbm_to_pct(-120), 0);
    }

    #[test]
    fn list_networks_parses_current_flag() {
        let r = "network id / ssid / bssid / flags\n0\tHome\tany\t[CURRENT]\n1\tCafe\tany\t\n2\tOffice\tany\t[DISABLED]\n";
        let l = parse_list_networks(r);
        assert_eq!(l.len(), 3);
        assert_eq!(l[0], SavedNetwork { id: 0, ssid: "Home".into(), current: true });
        assert!(!l[1].current);
        assert!(!l[2].current);
        assert!(parse_list_networks("network id / ssid / bssid / flags\n").is_empty());
    }
}
