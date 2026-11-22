//! The ssh time probe: argument construction, output parsing and the
//! RTT-corrected offset math are pure; only `probe_offset_ms` spawns ssh.

use crate::utils::config::ClockConfig;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const REMOTE_COMMAND: &str = "date +%s%3N";

/// The ssh argument vector for one probe of `host`.
pub fn ssh_args(cfg: &ClockConfig, host: &str) -> Vec<String> {
    let mut args = vec![
        "-i".to_string(),
        cfg.ssh_key_path.clone(),
        "-o".to_string(),
        "BatchMode=yes".to_string(),
        "-o".to_string(),
        format!("ConnectTimeout={}", cfg.ssh_connect_timeout_secs),
        "-o".to_string(),
        "StrictHostKeyChecking=accept-new".to_string(),
        "-o".to_string(),
        format!("UserKnownHostsFile={}", cfg.known_hosts_path),
    ];
    if cfg.ssh_port != 22 {
        args.push("-p".to_string());
        args.push(cfg.ssh_port.to_string());
    }
    args.push(format!("{}@{host}", cfg.ssh_user));
    args.push(REMOTE_COMMAND.to_string());
    args
}

/// Parses the remote `date +%s%3N` output (epoch milliseconds).
pub fn parse_epoch_ms(text: &str) -> Option<i64> {
    text.trim().parse().ok()
}

/// Offset in ms (remote minus local), assuming the remote clock was read
/// halfway through the round trip. `local_epoch_ms_at_t1` is the local time
/// when the reply arrived.
pub fn offset_ms(remote_epoch_ms: i64, rtt_ms: i64, local_epoch_ms_at_t1: i64) -> i64 {
    remote_epoch_ms + rtt_ms / 2 - local_epoch_ms_at_t1
}

/// Runs the ssh probe against `host`; `None` on any failure (spawn error,
/// bad exit, unparseable output, clock going backwards).
pub fn probe_offset_ms(cfg: &ClockConfig, host: &str) -> Option<i64> {
    let t0 = SystemTime::now();
    let output = Command::new("ssh").args(ssh_args(cfg, host)).output().ok()?;
    let t1 = SystemTime::now();
    if !output.status.success() {
        return None;
    }
    let remote_epoch_ms = parse_epoch_ms(&String::from_utf8_lossy(&output.stdout))?;
    let rtt_ms = t1.duration_since(t0).ok()?.as_millis() as i64;
    let local_ms = t1.duration_since(UNIX_EPOCH).ok()?.as_millis() as i64;
    Some(offset_ms(remote_epoch_ms, rtt_ms, local_ms))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_args_match_the_historical_command_line() {
        let args = ssh_args(&ClockConfig::default(), "10.42.1.1");
        assert_eq!(
            args,
            [
                "-i",
                "/var/lib/thing-revived/.ssh/id_ed25519",
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=3",
                "-o",
                "StrictHostKeyChecking=accept-new",
                "-o",
                "UserKnownHostsFile=/var/lib/thing-revived/.ssh/known_hosts",
                "andremmfaria@10.42.1.1",
                "date +%s%3N",
            ]
        );
    }

    #[test]
    fn non_default_port_adds_dash_p() {
        let cfg = ClockConfig { ssh_port: 2222, ..ClockConfig::default() };
        let args = ssh_args(&cfg, "h");
        let i = args.iter().position(|a| a == "-p").unwrap();
        assert_eq!(args[i + 1], "2222");
    }

    #[test]
    fn epoch_parsing() {
        assert_eq!(parse_epoch_ms("1700000000123\n"), Some(1_700_000_000_123));
        assert_eq!(parse_epoch_ms(" 5 "), Some(5));
        assert_eq!(parse_epoch_ms("Permission denied"), None);
        assert_eq!(parse_epoch_ms(""), None);
    }

    #[test]
    fn offset_math() {
        // Remote reads 1000 at the midpoint of a 40 ms round trip that ends at local 1010:
        // estimate at arrival = 1000 + 20 = 1020, so remote is 10 ms ahead.
        assert_eq!(offset_ms(1000, 40, 1010), 10);
        assert_eq!(offset_ms(1000, 0, 1000), 0);
        assert_eq!(offset_ms(1000, 41, 1020), 0); // odd rtt halves by integer division
        assert_eq!(offset_ms(900, 20, 1000), -90);
    }
}
