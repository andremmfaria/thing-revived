//! Wi-Fi status/scan/connect/forget for the settings menu. See docs/wifi.md.
//!
//! Backend choice: wpa_supplicant's unix-socket control interface (the same
//! protocol `wpa_cli` speaks), not its D-Bus API. Reasons: (1) persistence is
//! one documented command (`SAVE_CONFIG`) against the config file the
//! `thing-revived-wpa@.service` unit passes with `-c`, whereas the D-Bus
//! `AddNetwork` path has no save call this code could verify without hardware;
//! (2) the text protocol is trivially unit-testable with captured-format
//! fixtures, and the D-Bus dict-of-variants shapes are not; (3) no new crate
//! (std's `UnixDatagram` is enough) and no dependency on the D-Bus policy file
//! or bus activation. The cost is hand-parsing text replies, which is what the
//! pure functions below are tested for.
//!
//! This file has no WS/serde dependency, same split as bluetooth.rs: main.rs
//! owns the wire format and maps `WifiEvent` onto `BridgeEvent`.
//!
//! Secrets: a password only ever travels WS -> `connect` -> one
//! `SET_NETWORK ... psk "..."` datagram -> wpa_supplicant's config file. It is
//! never logged, never put in a `WifiEvent`, and error strings built here
//! never include the command text.

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use crate::utils::config::WifiConfig;
use log::{error, info, warn};

// Paths and timings (`[wifi]` in controller.toml, see `WifiConfig`). `/var`
// is the data partition on this image (docs/wifi.md), so state_dir and
// conf_path survive reboots while `/etc` does not. The `ctrl_interface=`
// directory holds the sockets, `<dir>/<ifname>`; `WPA_CTRL_DIR` overrides it
// so tests can point at a fake supplicant.

/// The control-socket directory: the `WPA_CTRL_DIR` env var wins over the
/// configured one.
fn ctrl_dir(cfg: &WifiConfig) -> PathBuf {
    std::env::var_os("WPA_CTRL_DIR").map_or_else(|| PathBuf::from(&cfg.ctrl_dir), PathBuf::from)
}
const WPA_CONF_CONTENT: &str = "ctrl_interface=/run/wpa_supplicant\nupdate_config=1\n";

// ---------------------------------------------------------------------------
// Pure data model and parsing
// ---------------------------------------------------------------------------

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

fn read_net_ifs(root: &Path) -> Vec<NetIf> {
    let Ok(rd) = fs::read_dir(root) else { return Vec::new() };
    rd.filter_map(|e| e.ok())
        .map(|e| {
            let p = e.path();
            let read = |f: &str| fs::read_to_string(p.join(f)).unwrap_or_default().trim().to_string();
            NetIf {
                name: e.file_name().to_string_lossy().into_owned(),
                if_type: read("type").parse().unwrap_or(0),
                operstate: read("operstate"),
                wireless: p.join("wireless").exists() || p.join("phy80211").exists(),
            }
        })
        .collect()
}

/// First wireless interface under `root` (`/sys/class/net` in production),
/// by sorted name so the choice is stable.
pub fn detect_interface_in(root: &Path) -> Option<String> {
    let mut names: Vec<String> = read_net_ifs(root).into_iter().filter(|i| i.wireless).map(|i| i.name).collect();
    names.sort();
    names.into_iter().next()
}

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

/// Why a connect attempt failed, with the stable `error` code the frontend
/// switches on and the user-facing message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FailKind {
    WrongPassword,
    NotFound,
    AssocRejected,
    AuthRejected,
    NoResponse,
    DhcpTimeout,
    Unavailable(String),
    InvalidInput(String),
}

impl FailKind {
    pub fn code(&self) -> &'static str {
        match self {
            FailKind::WrongPassword => "wrong_password",
            FailKind::NotFound => "network_not_found",
            FailKind::AssocRejected => "assoc_rejected",
            FailKind::AuthRejected => "auth_rejected",
            FailKind::NoResponse => "no_response",
            FailKind::DhcpTimeout => "dhcp_timeout",
            FailKind::Unavailable(_) => "unavailable",
            FailKind::InvalidInput(_) => "invalid_input",
        }
    }

    pub fn message(&self) -> String {
        match self {
            FailKind::WrongPassword => "Wrong password".into(),
            FailKind::NotFound => "Network not found (out of range?)".into(),
            FailKind::AssocRejected => "The access point rejected the connection".into(),
            FailKind::AuthRejected => "Authentication was rejected".into(),
            FailKind::NoResponse => "No response from the access point".into(),
            FailKind::DhcpTimeout => "Connected, but no IP address was assigned (DHCP timeout)".into(),
            FailKind::Unavailable(m) | FailKind::InvalidInput(m) => m.clone(),
        }
    }
}

/// Folds the event stream of one connect attempt into a verdict. Returns
/// `Some(Err)` as soon as a terminal failure is certain; `Connected` is
/// reported through `connected()` because success also needs an IP.
pub struct ConnectTracker {
    has_password: bool,
    not_found: u32,
    assoc_rejects: u32,
    connected: bool,
}

impl ConnectTracker {
    pub fn new(has_password: bool) -> Self {
        ConnectTracker { has_password, not_found: 0, assoc_rejects: 0, connected: false }
    }

    pub fn connected(&self) -> bool {
        self.connected
    }

    pub fn on_event(&mut self, ev: &WpaEvent) -> Option<FailKind> {
        match ev {
            WpaEvent::Connected => {
                self.connected = true;
                None
            }
            WpaEvent::HandshakeFailed if self.has_password && !self.connected => Some(FailKind::WrongPassword),
            WpaEvent::TempDisabled { reason } if !self.connected => match reason.as_str() {
                "WRONG_KEY" => Some(FailKind::WrongPassword),
                "AUTH_FAILED" | "CONN_FAILED" => {
                    Some(if self.has_password { FailKind::WrongPassword } else { FailKind::AuthRejected })
                }
                _ => None,
            },
            WpaEvent::NetworkNotFound if !self.connected => {
                self.not_found += 1;
                // The first miss can be a stale scan; the second is real.
                if self.not_found >= 2 {
                    Some(FailKind::NotFound)
                } else {
                    None
                }
            }
            WpaEvent::AssocReject if !self.connected => {
                self.assoc_rejects += 1;
                if self.assoc_rejects >= 3 {
                    Some(FailKind::AssocRejected)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Failure to report when the overall timeout elapses with no verdict.
    pub fn timeout_kind(&self) -> FailKind {
        if self.not_found > 0 {
            FailKind::NotFound
        } else {
            FailKind::NoResponse
        }
    }
}

// ---------------------------------------------------------------------------
// Control-socket client
// ---------------------------------------------------------------------------

static SOCK_COUNTER: AtomicU32 = AtomicU32::new(0);

/// A bound client datagram socket connected to wpa_supplicant's
/// per-interface control socket. Removes its own bind path on drop.
struct Ctrl {
    sock: UnixDatagram,
    local: PathBuf,
}

impl Ctrl {
    fn open(iface: &str, cfg: &WifiConfig) -> Result<Ctrl, String> {
        let local = PathBuf::from(format!(
            "/tmp/thing-revived-wpa-{}-{}",
            std::process::id(),
            SOCK_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_file(&local);
        let sock = UnixDatagram::bind(&local).map_err(|e| format!("bind control socket: {e}"))?;
        let ctrl = Ctrl { sock, local };
        ctrl.sock
            .connect(ctrl_dir(cfg).join(iface))
            .map_err(|e| format!("Wi-Fi service not running ({e})"))?;
        ctrl.sock
            .set_read_timeout(Some(Duration::from_secs(cfg.ctrl_reply_timeout_secs)))
            .map_err(|e| format!("set timeout: {e}"))?;
        Ok(ctrl)
    }

    /// Sends one command and returns the trimmed reply. The command is never
    /// echoed into an error, since it may carry a passphrase.
    fn request(&self, cmd: &str) -> Result<String, String> {
        self.sock.send(cmd.as_bytes()).map_err(|e| format!("send to wpa_supplicant: {e}"))?;
        let mut buf = vec![0u8; 16384];
        let n = self.sock.recv(&mut buf).map_err(|e| format!("no reply from wpa_supplicant: {e}"))?;
        Ok(String::from_utf8_lossy(&buf[..n]).into_owned())
    }

    /// Like `request` but the reply must be `OK`.
    fn ok(&self, what: &str, cmd: &str) -> Result<(), String> {
        let reply = self.request(cmd)?;
        if reply.trim() == "OK" {
            Ok(())
        } else {
            Err(format!("{what} failed ({})", reply.trim()))
        }
    }

    /// Subscribes this socket to unsolicited events.
    fn attach(iface: &str, cfg: &WifiConfig) -> Result<Ctrl, String> {
        let c = Ctrl::open(iface, cfg)?;
        c.ok("attach", "ATTACH")?;
        Ok(c)
    }

    fn next_event(&self, timeout: Duration) -> Option<WpaEvent> {
        self.sock.set_read_timeout(Some(timeout.max(Duration::from_millis(1)))).ok()?;
        let mut buf = vec![0u8; 4096];
        let n = self.sock.recv(&mut buf).ok()?;
        Some(parse_event(String::from_utf8_lossy(&buf[..n]).trim()))
    }
}

impl Drop for Ctrl {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.local);
    }
}

// ---------------------------------------------------------------------------
// Service object
// ---------------------------------------------------------------------------

/// Everything main.rs needs to put on the wire; no passwords in here.
#[derive(Clone, Debug)]
pub enum WifiEvent {
    Available { available: bool, interface: Option<String> },
    Status(StatusInfo),
    ScanStarted,
    ScanResults(Vec<NetworkInfo>),
    Connecting { ssid: String },
    ConnectResult { ssid: String, error: Option<FailKind> },
    SavedList(Vec<SavedNetwork>),
    Error { context: String, message: String },
}

struct BusyGuard<'a>(&'a AtomicBool);

impl<'a> BusyGuard<'a> {
    fn acquire(flag: &'a AtomicBool) -> Option<BusyGuard<'a>> {
        if flag.swap(true, Ordering::SeqCst) {
            None
        } else {
            Some(BusyGuard(flag))
        }
    }
}

impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

enum Target {
    New { ssid: String, password: String },
    Saved { ssid: String },
}

pub struct Wifi {
    cfg: WifiConfig,
    events: Sender<WifiEvent>,
    iface: Mutex<Option<String>>,
    last_status: Mutex<Option<StatusInfo>>,
    last_scan: Mutex<Vec<NetworkInfo>>,
    connecting: AtomicBool,
    scanning: AtomicBool,
}

impl Wifi {
    pub fn new(events: Sender<WifiEvent>) -> Wifi {
        Wifi::with_config(events, WifiConfig::default())
    }

    pub fn with_config(events: Sender<WifiEvent>, cfg: WifiConfig) -> Wifi {
        Wifi {
            cfg,
            events,
            iface: Mutex::new(None),
            last_status: Mutex::new(None),
            last_scan: Mutex::new(Vec::new()),
            connecting: AtomicBool::new(false),
            scanning: AtomicBool::new(false),
        }
    }

    fn emit(&self, e: WifiEvent) {
        let _ = self.events.send(e);
    }

    fn err(&self, context: &str, message: impl Into<String>) {
        self.emit(WifiEvent::Error { context: context.into(), message: message.into() });
    }

    fn open(&self, iface: &str) -> Result<Ctrl, String> {
        Ctrl::open(iface, &self.cfg)
    }

    fn attach(&self, iface: &str) -> Result<Ctrl, String> {
        Ctrl::attach(iface, &self.cfg)
    }

    fn net_class_dir(&self) -> PathBuf {
        PathBuf::from(&self.cfg.net_class_dir)
    }

    fn iface(&self) -> Option<String> {
        crate::sync_util::lock(&self.iface).clone()
    }

    /// Re-announces availability and status to a freshly connected client.
    pub fn announce(&self) {
        let iface = self.iface();
        self.emit(WifiEvent::Available { available: iface.is_some(), interface: iface });
        self.poll_status(true);
    }

    /// Polls for the adapter appearing/disappearing (USB hotplug, modeswitch
    /// re-enumeration), makes sure the supplicant is running for it, and
    /// pushes status changes. Never returns.
    pub fn run_watcher(&self) {
        let mut last_start_attempt: Option<Instant> = None;
        let service_start_retry = Duration::from_secs(self.cfg.service_start_retry_secs);
        let poll_interval = Duration::from_secs(self.cfg.poll_interval_secs);
        loop {
            let now = detect_interface_in(&self.net_class_dir());
            let changed = {
                let mut cur = crate::sync_util::lock(&self.iface);
                if *cur != now {
                    *cur = now.clone();
                    true
                } else {
                    false
                }
            };
            if changed {
                info!("wifi: interface is now {now:?}");
                self.emit(WifiEvent::Available { available: now.is_some(), interface: now.clone() });
                *crate::sync_util::lock(&self.last_status) = None;
                if now.is_none() {
                    crate::sync_util::lock(&self.last_scan).clear();
                }
            }
            if let Some(ifname) = &now {
                // The udev rule normally started the unit already; this is
                // the fallback for a rule that did not fire (docs/wifi.md).
                if !ctrl_dir(&self.cfg).join(ifname).exists()
                    && last_start_attempt.is_none_or(|t| t.elapsed() >= service_start_retry)
                {
                    last_start_attempt = Some(Instant::now());
                    ensure_config(&self.cfg);
                    start_service(ifname);
                }
                self.poll_status(false);
            } else {
                last_start_attempt = None;
            }
            thread::sleep(poll_interval);
        }
    }

    fn read_status(&self, ctrl: &Ctrl, iface: &str) -> Result<StatusInfo, String> {
        let st = parse_status(&ctrl.request("STATUS")?);
        let state = derive_state(&st);
        let signal_pct = if state == ConnState::Disconnected {
            None
        } else {
            ctrl.request("SIGNAL_POLL").ok().and_then(|r| parse_rssi(&r)).map(dbm_to_pct)
        };
        let ethernet_up = ethernet_is_up(&read_net_ifs(&self.net_class_dir()));
        Ok(StatusInfo {
            state,
            ssid: if state == ConnState::Disconnected { None } else { st.ssid },
            ip: st.ip_address.filter(|s| !s.is_empty()),
            interface: iface.to_string(),
            signal_pct,
            ethernet_up,
        })
    }

    /// Reads status and emits it if it changed (or always when `force`).
    /// A not-yet-running supplicant is a normal transient state and yields
    /// a disconnected status rather than an error toast.
    fn poll_status(&self, force: bool) {
        let Some(iface) = self.iface() else { return };
        let info = match self.open(&iface).and_then(|c| self.read_status(&c, &iface)) {
            Ok(i) => i,
            Err(_) => StatusInfo {
                state: ConnState::Disconnected,
                ssid: None,
                ip: None,
                interface: iface.clone(),
                signal_pct: None,
                ethernet_up: ethernet_is_up(&read_net_ifs(&self.net_class_dir())),
            },
        };
        let mut last = crate::sync_util::lock(&self.last_status);
        if force || last.as_ref() != Some(&info) {
            *last = Some(info.clone());
            drop(last);
            self.emit(WifiEvent::Status(info));
        }
    }

    pub fn list_saved(&self) {
        match self.saved_networks() {
            Ok(list) => self.emit(WifiEvent::SavedList(list)),
            Err(m) => self.err("wifi_saved_list_get", m),
        }
    }

    fn saved_networks(&self) -> Result<Vec<SavedNetwork>, String> {
        let iface = self.iface().ok_or("No Wi-Fi adapter")?;
        let ctrl = self.open(&iface)?;
        Ok(parse_list_networks(&ctrl.request("LIST_NETWORKS")?))
    }

    pub fn disconnect(&self) {
        let Some(iface) = self.iface() else { return self.err("wifi_disconnect", "No Wi-Fi adapter") };
        match self.open(&iface).and_then(|c| c.ok("disconnect", "DISCONNECT")) {
            Ok(()) => self.poll_status(true),
            Err(m) => self.err("wifi_disconnect", m),
        }
    }

    pub fn forget(&self, ssid: &str) {
        let result = (|| -> Result<(), String> {
            let iface = self.iface().ok_or("No Wi-Fi adapter")?;
            let ctrl = self.open(&iface)?;
            let ids: Vec<u32> = parse_list_networks(&ctrl.request("LIST_NETWORKS")?)
                .into_iter()
                .filter(|n| n.ssid == ssid)
                .map(|n| n.id)
                .collect();
            if ids.is_empty() {
                return Err("Network is not saved".into());
            }
            for id in ids {
                ctrl.ok("remove network", &format!("REMOVE_NETWORK {id}"))?;
            }
            ctrl.ok("save config", "SAVE_CONFIG")
        })();
        match result {
            Ok(()) => {
                self.list_saved();
                self.poll_status(true);
            }
            Err(m) => self.err("wifi_forget", m),
        }
    }

    pub fn scan(&self) {
        let Some(_guard) = BusyGuard::acquire(&self.scanning) else { return };
        let Some(iface) = self.iface() else { return self.err("wifi_scan", "No Wi-Fi adapter") };
        self.emit(WifiEvent::ScanStarted);
        let result = (|| -> Result<Vec<NetworkInfo>, String> {
            let mon = self.attach(&iface)?;
            let ctrl = self.open(&iface)?;
            // FAIL-BUSY means a scan is already running; its results are
            // just as good, so any reply is fine and we wait for the event.
            let _ = ctrl.request("SCAN")?;
            let deadline = Instant::now() + Duration::from_secs(self.cfg.scan_wait_secs);
            while let Some(left) = deadline.checked_duration_since(Instant::now()) {
                match mon.next_event(left) {
                    Some(WpaEvent::ScanResults) | Some(WpaEvent::ScanFailed) => break,
                    Some(_) => continue,
                    None => break,
                }
            }
            Ok(parse_scan_results(&ctrl.request("SCAN_RESULTS")?))
        })();
        match result {
            Ok(list) => {
                *crate::sync_util::lock(&self.last_scan) = list.clone();
                self.emit(WifiEvent::ScanResults(list));
            }
            Err(m) => self.err("wifi_scan", m),
        }
    }

    pub fn connect(&self, ssid: String, password: String) {
        self.run_connect(Target::New { ssid, password });
    }

    pub fn connect_saved(&self, ssid: String) {
        self.run_connect(Target::Saved { ssid });
    }

    fn run_connect(&self, target: Target) {
        let ssid = match &target {
            Target::New { ssid, .. } | Target::Saved { ssid } => ssid.clone(),
        };
        let Some(_guard) = BusyGuard::acquire(&self.connecting) else {
            return self.err("wifi_connect", "A connection attempt is already in progress");
        };
        self.emit(WifiEvent::Connecting { ssid: ssid.clone() });
        let error = self.connect_flow(target).err();
        if let Some(e) = &error {
            warn!("wifi: connect to {ssid:?} failed: {}", e.code());
        }
        self.emit(WifiEvent::ConnectResult { ssid, error });
        self.poll_status(true);
        self.list_saved();
    }

    fn connect_flow(&self, target: Target) -> Result<(), FailKind> {
        let unavailable = |m: String| FailKind::Unavailable(m);
        let iface = self.iface().ok_or_else(|| unavailable("No Wi-Fi adapter".into()))?;
        let mon = self.attach(&iface).map_err(&unavailable)?;
        let ctrl = self.open(&iface).map_err(&unavailable)?;

        let (ssid, new_id, has_password, old_ids) = match &target {
            Target::New { ssid, password } => {
                validate_ssid(ssid).map_err(FailKind::InvalidInput)?;
                // Security comes from our own last scan, never from the client.
                let security = self
                    .last_scan
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .iter()
                    .find(|n| &n.ssid == ssid)
                    .map(|n| n.security)
                    .unwrap_or(if password.is_empty() { Security::Open } else { Security::Psk });
                validate_password(password, security).map_err(FailKind::InvalidInput)?;

                let saved = parse_list_networks(&ctrl.request("LIST_NETWORKS").map_err(&unavailable)?);
                let old_ids: Vec<u32> = saved.iter().filter(|n| &n.ssid == ssid).map(|n| n.id).collect();

                let reply = ctrl.request("ADD_NETWORK").map_err(&unavailable)?;
                let id: u32 = reply
                    .trim()
                    .parse()
                    .map_err(|_| unavailable("wpa_supplicant refused to add a network".into()))?;
                let configured = (|| -> Result<(), String> {
                    for (k, v) in network_settings(ssid, security) {
                        ctrl.ok("configure network", &format!("SET_NETWORK {id} {k} {v}"))?;
                    }
                    if security.needs_password() {
                        ctrl.ok("configure network", &set_psk_command(id, password))?;
                    }
                    Ok(())
                })();
                if let Err(m) = configured {
                    let _ = ctrl.request(&format!("REMOVE_NETWORK {id}"));
                    return Err(unavailable(m));
                }
                (ssid.clone(), id, security.needs_password(), old_ids)
            }
            Target::Saved { ssid } => {
                let saved = parse_list_networks(&ctrl.request("LIST_NETWORKS").map_err(&unavailable)?);
                let id = saved
                    .iter()
                    .find(|n| &n.ssid == ssid)
                    .map(|n| n.id)
                    .ok_or_else(|| FailKind::InvalidInput("Network is not saved".into()))?;
                // A saved network is assumed to need a password; the only
                // effect is mapping auth failures to "wrong password".
                (ssid.clone(), id, true, Vec::new())
            }
        };
        let is_new = matches!(target, Target::New { .. });

        // Re-selecting the network we are already on emits no CONNECTED
        // event, so for saved networks an existing association counts.
        let already_on = if is_new { None } else { Some(ssid.as_str()) };
        let outcome = self.drive_connection(&ctrl, &mon, new_id, has_password, already_on);

        match &outcome {
            Ok(()) => {
                if is_new {
                    for id in old_ids {
                        let _ = ctrl.request(&format!("REMOVE_NETWORK {id}"));
                    }
                }
                // SELECT_NETWORK disabled every other saved network; undo
                // that before saving or they would not auto-connect at boot.
                let _ = ctrl.request("ENABLE_NETWORK all");
                if is_new {
                    if let Err(m) = ctrl.ok("save config", "SAVE_CONFIG") {
                        error!("wifi: SAVE_CONFIG after connecting to {ssid:?} failed: {m}");
                        self.err("wifi_connect", "Connected, but saving the network failed");
                    }
                }
            }
            Err(_) => {
                if is_new {
                    let _ = ctrl.request(&format!("REMOVE_NETWORK {new_id}"));
                }
                let _ = ctrl.request("ENABLE_NETWORK all");
            }
        }
        outcome
    }

    fn drive_connection(
        &self,
        ctrl: &Ctrl,
        mon: &Ctrl,
        id: u32,
        has_password: bool,
        already_on: Option<&str>,
    ) -> Result<(), FailKind> {
        ctrl.ok("select network", &format!("SELECT_NETWORK {id}")).map_err(FailKind::Unavailable)?;
        let mut tracker = ConnectTracker::new(has_password);
        let deadline = Instant::now() + Duration::from_secs(self.cfg.connect_timeout_secs);
        while !tracker.connected() {
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                return Err(tracker.timeout_kind());
            };
            if let Some(ev) = mon.next_event(left.min(Duration::from_secs(1))) {
                if let Some(fail) = tracker.on_event(&ev) {
                    return Err(fail);
                }
            } else if let Some(ssid) = already_on {
                let st = parse_status(&ctrl.request("STATUS").map_err(FailKind::Unavailable)?);
                if st.wpa_state == "COMPLETED" && st.ssid.as_deref() == Some(ssid) {
                    tracker.on_event(&WpaEvent::Connected);
                }
            }
        }
        // Associated; success also needs an address from DHCP (networkd).
        let dhcp_deadline = Instant::now() + Duration::from_secs(self.cfg.dhcp_timeout_secs);
        loop {
            let st = parse_status(&ctrl.request("STATUS").map_err(FailKind::Unavailable)?);
            if derive_state(&st) == ConnState::Connected {
                return Ok(());
            }
            if Instant::now() >= dhcp_deadline {
                return Err(FailKind::DhcpTimeout);
            }
            thread::sleep(Duration::from_millis(500));
        }
    }
}

/// Creates the root-only state dir and a minimal wpa_supplicant.conf if
/// they are missing. The tmpfiles.d entry normally does this at boot; this
/// covers a first run before it has applied. Existing files are never
/// touched so saved networks survive.
pub fn ensure_config(cfg: &WifiConfig) {
    let (state_dir, conf_path) = (&cfg.state_dir, &cfg.conf_path);
    let mut db = fs::DirBuilder::new();
    db.recursive(true).mode(0o700);
    if let Err(e) = db.create(state_dir) {
        error!("wifi: cannot create {state_dir}: {e}");
        return;
    }
    match fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(conf_path) {
        Ok(mut f) => {
            if let Err(e) = f.write_all(WPA_CONF_CONTENT.as_bytes()) {
                error!("wifi: cannot write {conf_path}: {e}");
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => error!("wifi: cannot create {conf_path}: {e}"),
    }
}

fn start_service(iface: &str) {
    // Interface names come from /sys, but keep the unit instance name strict
    // anyway since it goes into a command line.
    if iface.is_empty() || !iface.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return;
    }
    let unit = format!("thing-revived-wpa@{iface}.service");
    match Command::new("systemctl").args(["start", "--no-block", &unit]).status() {
        Ok(s) if s.success() => info!("wifi: requested start of {unit}"),
        Ok(s) => warn!("wifi: systemctl start {unit} exited with {s}"),
        Err(e) => error!("wifi: failed to spawn systemctl start {unit}: {e}"),
    }
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

    #[test]
    fn interface_detection_uses_sysfs_markers() {
        let root = std::env::temp_dir().join(format!("tr-wifi-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("eth0")).unwrap();
        fs::create_dir_all(root.join("wlan0/wireless")).unwrap();
        fs::create_dir_all(root.join("wlan1")).unwrap();
        fs::write(root.join("wlan1/phy80211"), "").unwrap();
        assert_eq!(detect_interface_in(&root).as_deref(), Some("wlan0"));
        fs::remove_dir_all(root.join("wlan0")).unwrap();
        assert_eq!(detect_interface_in(&root).as_deref(), Some("wlan1"));
        fs::remove_dir_all(root.join("wlan1")).unwrap();
        assert_eq!(detect_interface_in(&root), None);
        assert_eq!(detect_interface_in(&root.join("missing")), None);
        let _ = fs::remove_dir_all(&root);
    }

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

    #[test]
    fn tracker_reports_wrong_password() {
        let mut t = ConnectTracker::new(true);
        assert_eq!(t.on_event(&WpaEvent::Other), None);
        assert_eq!(
            t.on_event(&WpaEvent::TempDisabled { reason: "WRONG_KEY".into() }),
            Some(FailKind::WrongPassword)
        );
        let mut t = ConnectTracker::new(true);
        assert_eq!(t.on_event(&WpaEvent::HandshakeFailed), Some(FailKind::WrongPassword));
        let mut t = ConnectTracker::new(false);
        assert_eq!(
            t.on_event(&WpaEvent::TempDisabled { reason: "CONN_FAILED".into() }),
            Some(FailKind::AuthRejected)
        );
    }

    #[test]
    fn tracker_not_found_needs_two_misses_and_reject_needs_three() {
        let mut t = ConnectTracker::new(true);
        assert_eq!(t.on_event(&WpaEvent::NetworkNotFound), None);
        assert_eq!(t.on_event(&WpaEvent::NetworkNotFound), Some(FailKind::NotFound));
        let mut t = ConnectTracker::new(true);
        assert_eq!(t.on_event(&WpaEvent::AssocReject), None);
        assert_eq!(t.on_event(&WpaEvent::AssocReject), None);
        assert_eq!(t.on_event(&WpaEvent::AssocReject), Some(FailKind::AssocRejected));
    }

    #[test]
    fn tracker_connected_stops_failure_reports_and_timeout_kind() {
        let mut t = ConnectTracker::new(true);
        assert_eq!(t.timeout_kind(), FailKind::NoResponse);
        t.on_event(&WpaEvent::NetworkNotFound);
        assert_eq!(t.timeout_kind(), FailKind::NotFound);
        t.on_event(&WpaEvent::Connected);
        assert!(t.connected());
        assert_eq!(t.on_event(&WpaEvent::TempDisabled { reason: "WRONG_KEY".into() }), None);
    }

    #[test]
    fn fail_kinds_have_stable_codes_and_no_password_text() {
        assert_eq!(FailKind::WrongPassword.code(), "wrong_password");
        assert_eq!(FailKind::DhcpTimeout.code(), "dhcp_timeout");
        assert_eq!(FailKind::NoResponse.code(), "no_response");
        assert!(FailKind::DhcpTimeout.message().contains("DHCP"));
    }

    // --- end-to-end against a fake wpa_supplicant control socket ----------

    use std::sync::{Arc, Mutex as StdMutex};
    use std::sync::mpsc::channel;

    /// Minimal stand-in for wpa_supplicant's ctrl socket: answers the
    /// commands the flows use and emits scripted events after SELECT_NETWORK.
    struct FakeWpa {
        log: Arc<StdMutex<Vec<String>>>,
    }

    impl FakeWpa {
        fn start(dir: &Path, select_events: Vec<&'static str>) -> FakeWpa {
            fs::create_dir_all(dir).unwrap();
            let path = dir.join("wlan0");
            let _ = fs::remove_file(&path);
            let sock = UnixDatagram::bind(&path).unwrap();
            let log = Arc::new(StdMutex::new(Vec::new()));
            let log2 = log.clone();
            thread::spawn(move || {
                let mut attached: Option<std::os::unix::net::SocketAddr> = None;
                let mut buf = [0u8; 4096];
                while let Ok((n, from)) = sock.recv_from(&mut buf) {
                    let cmd = String::from_utf8_lossy(&buf[..n]).to_string();
                    log2.lock().unwrap().push(cmd.clone());
                    let reply: String = match cmd.split(' ').next().unwrap() {
                        "ATTACH" => {
                            attached = Some(from.clone());
                            "OK\n".into()
                        }
                        "LIST_NETWORKS" => "network id / ssid / bssid / flags\n0\tHome\tany\t[CURRENT]\n1\tOld\tany\t\n".into(),
                        "ADD_NETWORK" => "2\n".into(),
                        "STATUS" => "wpa_state=COMPLETED\nssid=Home\nip_address=10.0.0.9\n".into(),
                        "SCAN_RESULTS" => "bssid / frequency / signal level / flags / ssid\naa\t2412\t-60\t[WPA2-PSK-CCMP][ESS]\tHome\n".into(),
                        _ => "OK\n".into(),
                    };
                    let _ = sock.send_to_addr(reply.as_bytes(), &from);
                    let evs: Vec<&str> = match cmd.split(' ').next().unwrap() {
                        "SELECT_NETWORK" => select_events.clone(),
                        "SCAN" => vec!["<3>CTRL-EVENT-SCAN-RESULTS "],
                        _ => vec![],
                    };
                    if let Some(a) = &attached {
                        for e in evs {
                            let _ = sock.send_to_addr(e.as_bytes(), a);
                        }
                    }
                }
            });
            FakeWpa { log }
        }

        fn commands(&self) -> Vec<String> {
            self.log.lock().unwrap().clone()
        }
    }

    fn fake_env(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tr-fakewpa-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        // Tests run on parallel threads and share the process env, so every
        // fake uses the same variable only through this serialized helper.
        dir
    }

    static ENV_LOCK: StdMutex<()> = StdMutex::new(());

    fn wifi_for(dir: &Path) -> (Wifi, std::sync::mpsc::Receiver<WifiEvent>) {
        std::env::set_var("WPA_CTRL_DIR", dir);
        let (tx, rx) = channel();
        let w = Wifi::new(tx);
        *w.iface.lock().unwrap() = Some("wlan0".into());
        (w, rx)
    }

    #[test]
    fn connect_success_saves_config_and_replaces_old_entry() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = fake_env("ok");
        let fake = FakeWpa::start(&dir, vec!["<3>CTRL-EVENT-CONNECTED - Connection to aa completed"]);
        let (w, rx) = wifi_for(&dir);
        *w.last_scan.lock().unwrap() = parse_scan_results(SCAN);
        w.connect("Home".into(), "correct horse".into());
        let events: Vec<WifiEvent> = rx.try_iter().collect();
        assert!(events.iter().any(|e| matches!(e, WifiEvent::ConnectResult { error: None, .. })), "{events:?}");
        let cmds = fake.commands();
        assert!(cmds.contains(&"SET_NETWORK 2 ssid 486f6d65".to_string()), "{cmds:?}");
        assert!(cmds.contains(&"SET_NETWORK 2 key_mgmt WPA-PSK".to_string()));
        assert!(cmds.contains(&"SET_NETWORK 2 psk \"correct horse\"".to_string()));
        assert!(cmds.contains(&"SELECT_NETWORK 2".to_string()));
        assert!(cmds.contains(&"REMOVE_NETWORK 0".to_string()), "old entry for the same SSID replaced");
        assert!(!cmds.contains(&"REMOVE_NETWORK 1".to_string()), "other SSIDs untouched");
        let en = cmds.iter().position(|c| c == "ENABLE_NETWORK all").unwrap();
        let save = cmds.iter().position(|c| c == "SAVE_CONFIG").unwrap();
        assert!(en < save, "re-enable others before saving so they keep auto-connecting");
        // The password must not appear in anything sent to clients.
        assert!(!format!("{events:?}").contains("correct horse"));
    }

    #[test]
    fn connect_wrong_password_removes_attempt_and_does_not_save() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = fake_env("bad");
        let fake = FakeWpa::start(
            &dir,
            vec!["<3>CTRL-EVENT-SSID-TEMP-DISABLED id=2 ssid=\"Home\" auth_failures=1 duration=10 reason=WRONG_KEY"],
        );
        let (w, rx) = wifi_for(&dir);
        *w.last_scan.lock().unwrap() = parse_scan_results(SCAN);
        w.connect("Home".into(), "wrong password".into());
        let events: Vec<WifiEvent> = rx.try_iter().collect();
        assert!(
            events.iter().any(|e| matches!(e, WifiEvent::ConnectResult { error: Some(FailKind::WrongPassword), .. })),
            "{events:?}"
        );
        let cmds = fake.commands();
        assert!(cmds.contains(&"REMOVE_NETWORK 2".to_string()));
        assert!(!cmds.contains(&"REMOVE_NETWORK 0".to_string()), "previous entry survives a failed attempt");
        assert!(!cmds.contains(&"SAVE_CONFIG".to_string()));
        assert!(!format!("{events:?}").contains("wrong password"));
    }

    #[test]
    fn connect_rejects_bad_password_before_touching_wpa() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = fake_env("short");
        let fake = FakeWpa::start(&dir, vec![]);
        let (w, rx) = wifi_for(&dir);
        *w.last_scan.lock().unwrap() = parse_scan_results(SCAN);
        w.connect("Home".into(), "short".into());
        let events: Vec<WifiEvent> = rx.try_iter().collect();
        assert!(events.iter().any(|e| matches!(e, WifiEvent::ConnectResult { error: Some(FailKind::InvalidInput(_)), .. })));
        assert!(!fake.commands().iter().any(|c| c.starts_with("ADD_NETWORK")));
    }

    #[test]
    fn scan_waits_for_event_then_reads_results() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = fake_env("scan");
        let fake = FakeWpa::start(&dir, vec![]);
        let (w, rx) = wifi_for(&dir);
        w.scan();
        let events: Vec<WifiEvent> = rx.try_iter().collect();
        assert!(matches!(events[0], WifiEvent::ScanStarted));
        assert!(matches!(&events[1], WifiEvent::ScanResults(l) if l.len() == 1 && l[0].ssid == "Home"));
        let cmds = fake.commands();
        assert!(cmds.iter().position(|c| c == "SCAN").unwrap() < cmds.iter().position(|c| c == "SCAN_RESULTS").unwrap());
    }

    #[test]
    fn forget_removes_matching_ids_and_saves() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = fake_env("forget");
        let fake = FakeWpa::start(&dir, vec![]);
        let (w, rx) = wifi_for(&dir);
        w.forget("Old");
        let cmds = fake.commands();
        assert!(cmds.contains(&"REMOVE_NETWORK 1".to_string()));
        assert!(!cmds.contains(&"REMOVE_NETWORK 0".to_string()));
        assert!(cmds.contains(&"SAVE_CONFIG".to_string()));
        assert!(rx.try_iter().any(|e| matches!(e, WifiEvent::SavedList(_))));
        w.forget("Nope");
        assert!(rx.try_iter().any(|e| matches!(e, WifiEvent::Error { .. })));
    }

    #[test]
    fn missing_supplicant_reports_unavailable_not_panic() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = fake_env("none");
        fs::create_dir_all(&dir).unwrap();
        let (w, rx) = wifi_for(&dir);
        w.connect("Home".into(), "correct horse".into());
        let events: Vec<WifiEvent> = rx.try_iter().collect();
        assert!(events.iter().any(|e| matches!(e, WifiEvent::ConnectResult { error: Some(FailKind::Unavailable(_)), .. })));
    }
}
