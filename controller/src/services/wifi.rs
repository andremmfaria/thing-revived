//! Wi-Fi status/scan/connect/forget service for the settings menu. See
//! docs/wifi.md. No WS/serde dependency: `app::wire_map` maps `WifiEvent`
//! onto `BridgeEvent`.
//!
//! Secrets: a password only ever travels WS -> `connect` -> one
//! `SET_NETWORK ... psk "..."` datagram -> wpa_supplicant's config file. It is
//! never logged, never put in a `WifiEvent`, and error strings built here
//! never include the command text.

use crate::adapters::wpa_ctrl::{ctrl_dir, detect_interface_in, ensure_config, read_net_ifs, start_service, Ctrl};
use crate::domain::wifi::*;
use crate::utils::config::WifiConfig;
use log::{error, info, warn};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::net::UnixDatagram;
    use std::path::Path;

    const SCAN: &str = "bssid / frequency / signal level / flags / ssid\n\
aa:bb:cc:00:00:01\t2412\t-70\t[WPA2-PSK-CCMP][ESS]\tHome\n\
aa:bb:cc:00:00:02\t5180\t-52\t[WPA2-PSK-CCMP][ESS]\tHome\n\
aa:bb:cc:00:00:03\t2437\t-60\t[RSN-SAE-CCMP][ESS]\tSecure3\n\
aa:bb:cc:00:00:04\t2437\t-85\t[ESS]\tCafe Free\n\
aa:bb:cc:00:00:05\t2437\t-40\t[WPA2-PSK-CCMP][ESS]\t\n\
aa:bb:cc:00:00:06\t2462\t-90\t[WEP][ESS]\tOld\n\
aa:bb:cc:00:00:07\t2462\t-65\t[WPA2-EAP-CCMP][ESS]\tCorp\n\
aa:bb:cc:00:00:08\t2462\t-80\t[WPA2-PSK-CCMP][RSN-SAE-CCMP][ESS]\tMixed\n";

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
