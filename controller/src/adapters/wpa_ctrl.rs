//! wpa_supplicant control-socket client and the sysfs/filesystem helpers the
//! Wi-Fi service needs. Secrets: a password only ever travels as one
//! `SET_NETWORK ... psk "..."` datagram; it is never logged and error strings
//! built here never include the command text.

use crate::domain::wifi::{parse_event, NetIf, WpaEvent};
use crate::utils::config::WifiConfig;
use log::{error, info, warn};
use std::fs;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

// Paths and timings (`[wifi]` in controller.toml, see `WifiConfig`). `/var`
// is the data partition on this image (docs/wifi.md), so state_dir and
// conf_path survive reboots while `/etc` does not. The `ctrl_interface=`
// directory holds the sockets, `<dir>/<ifname>`; `WPA_CTRL_DIR` overrides it
// so tests can point at a fake supplicant.

/// The control-socket directory: the `WPA_CTRL_DIR` env var wins over the
/// configured one.
pub fn ctrl_dir(cfg: &WifiConfig) -> PathBuf {
    std::env::var_os("WPA_CTRL_DIR").map_or_else(|| PathBuf::from(&cfg.ctrl_dir), PathBuf::from)
}
const WPA_CONF_CONTENT: &str = "ctrl_interface=/run/wpa_supplicant\nupdate_config=1\n";

// ---------------------------------------------------------------------------
// Control-socket client
// ---------------------------------------------------------------------------

static SOCK_COUNTER: AtomicU32 = AtomicU32::new(0);

/// A bound client datagram socket connected to wpa_supplicant's
/// per-interface control socket. Removes its own bind path on drop.
pub struct Ctrl {
    sock: UnixDatagram,
    local: PathBuf,
}

impl Ctrl {
    pub fn open(iface: &str, cfg: &WifiConfig) -> Result<Ctrl, String> {
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
    pub fn request(&self, cmd: &str) -> Result<String, String> {
        self.sock.send(cmd.as_bytes()).map_err(|e| format!("send to wpa_supplicant: {e}"))?;
        let mut buf = vec![0u8; 16384];
        let n = self.sock.recv(&mut buf).map_err(|e| format!("no reply from wpa_supplicant: {e}"))?;
        Ok(String::from_utf8_lossy(&buf[..n]).into_owned())
    }

    /// Like `request` but the reply must be `OK`.
    pub fn ok(&self, what: &str, cmd: &str) -> Result<(), String> {
        let reply = self.request(cmd)?;
        if reply.trim() == "OK" {
            Ok(())
        } else {
            Err(format!("{what} failed ({})", reply.trim()))
        }
    }

    /// Subscribes this socket to unsolicited events.
    pub fn attach(iface: &str, cfg: &WifiConfig) -> Result<Ctrl, String> {
        let c = Ctrl::open(iface, cfg)?;
        c.ok("attach", "ATTACH")?;
        Ok(c)
    }

    pub fn next_event(&self, timeout: Duration) -> Option<WpaEvent> {
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

pub fn read_net_ifs(root: &Path) -> Vec<NetIf> {
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

pub fn start_service(iface: &str) {
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
}
