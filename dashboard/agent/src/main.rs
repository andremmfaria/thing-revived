//! noc-agent: on-device metrics collector for the NOC dashboard.
//!
//! Rust port of the original `noc-agent.py` (deleted; see docs/dashboard.md).
//! Every `INTERVAL` seconds, reads /proc/stat, /proc/meminfo, /proc/diskstats,
//! /proc/loadavg, runs `df -k` and `hciconfig -a hci0`, computes the same
//! CPU%/IO-rate deltas, keeps the same 60-sample rolling history per metric,
//! and atomically writes the result (tmp file + rename) to
//! /var/thing-revived/noc-metrics.json. The JSON field names/types below are
//! kept byte-for-byte identical to the Python original so that the existing,
//! untouched `dashboard/frontend/app.js` keeps working with zero changes.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const INTERVAL_S: f64 = 2.0;
// Separate, reboot-persistent writable mount (see docs/dashboard.md): the
// root fs (which carries /usr/share and /opt) remounts read-only on every
// boot, but /var stays rw with no remount needed.
const OUTPUT_PATH: &str = "/var/thing-revived/noc-metrics.json";
const HISTORY_LEN: usize = 60; // ~2 minutes of history at the default interval

const DISK_MOUNTS: [&str; 4] = ["/", "/var", "/var/lib", "/home"];
const IO_DEVICES: [&str; 1] = ["mmcblk0"];
const NET_IFACES_OF_INTEREST: [&str; 4] = ["bnep0", "usb0", "eth0", "wlan0"];

/// Per-cpu-line raw /proc/stat counters: user, nice, system, idle, iowait,
/// irq, softirq, steal (first 8 fields after the "cpuN" label).
type CpuLine = [u64; 8];

fn read_proc_stat() -> HashMap<String, CpuLine> {
    let mut cpus = HashMap::new();
    let content = fs::read_to_string("/proc/stat").unwrap_or_default();
    for line in content.lines() {
        if !line.starts_with("cpu") {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.is_empty() {
            continue;
        }
        let label = parts[0].to_string();
        let mut nums: CpuLine = [0; 8];
        let mut ok = true;
        for i in 0..8 {
            match parts.get(i + 1).and_then(|s| s.parse::<u64>().ok()) {
                Some(v) => nums[i] = v,
                None => {
                    ok = false;
                    break;
                }
            }
        }
        if ok {
            cpus.insert(label, nums);
        }
    }
    cpus
}

/// Percent busy between two /proc/stat samples for one cpu line, the
/// standard `(total_delta - idle_delta) / total_delta` method.
fn cpu_pct(prev: &CpuLine, curr: &CpuLine) -> f64 {
    let prev_idle = (prev[3] + prev[4]) as i64;
    let curr_idle = (curr[3] + curr[4]) as i64;
    let prev_total: i64 = prev.iter().map(|&v| v as i64).sum();
    let curr_total: i64 = curr.iter().map(|&v| v as i64).sum();
    let total_delta = curr_total - prev_total;
    let idle_delta = curr_idle - prev_idle;
    if total_delta <= 0 {
        return 0.0;
    }
    (100.0 * (total_delta - idle_delta) as f64 / total_delta as f64).clamp(0.0, 100.0)
}

struct MemInfo {
    total_kb: u64,
    used_kb: u64,
    available_kb: u64,
    cached_kb: u64,
    buffers_kb: u64,
    pct: f64,
}

fn read_meminfo() -> MemInfo {
    let content = fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let mut info: HashMap<String, u64> = HashMap::new();
    for line in content.lines() {
        if let Some((key, rest)) = line.split_once(':') {
            let rest = rest.trim().trim_end_matches("kB").trim();
            if let Ok(v) = rest.parse::<u64>() {
                info.insert(key.to_string(), v);
            }
        }
    }
    let total = *info.get("MemTotal").unwrap_or(&0);
    let available = *info.get("MemAvailable").unwrap_or(&0);
    let free = *info.get("MemFree").unwrap_or(&0);
    let buffers = *info.get("Buffers").unwrap_or(&0);
    let cached = *info.get("Cached").unwrap_or(&0);
    let used = total.saturating_sub(free).saturating_sub(buffers).saturating_sub(cached);
    let pct = if total > 0 {
        round1(100.0 * used as f64 / total as f64)
    } else {
        0.0
    };
    MemInfo {
        total_kb: total,
        used_kb: used,
        available_kb: available,
        cached_kb: cached,
        buffers_kb: buffers,
        pct,
    }
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

fn run_capture(cmd: &str, args: &[&str]) -> String {
    Command::new(cmd)
        .args(args)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

fn read_disk_usage() -> Vec<Value> {
    let out = run_capture("df", &["-k"]);
    let mut rows: Vec<Value> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for line in out.lines().skip(1) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 6 {
            continue;
        }
        let (filesystem, size_kb, used_kb, avail_kb, pct, mount) =
            (parts[0], parts[1], parts[2], parts[3], parts[4], parts[5]);
        if !DISK_MOUNTS.contains(&mount) {
            continue;
        }
        let key = (filesystem.to_string(), mount.to_string());
        if !seen.insert(key) {
            continue;
        }
        let (size_kb_i, used_kb_i, avail_kb_i) = match (
            size_kb.parse::<i64>(),
            used_kb.parse::<i64>(),
            avail_kb.parse::<i64>(),
        ) {
            (Ok(a), Ok(b), Ok(c)) => (a, b, c),
            _ => continue,
        };
        let pct_str = pct.trim_end_matches('%');
        let pct_i: i64 = pct_str.parse().unwrap_or(0);
        rows.push(json!({
            "mount": mount,
            "filesystem": filesystem,
            "size_kb": size_kb_i,
            "used_kb": used_kb_i,
            "avail_kb": avail_kb_i,
            "pct": pct_i,
        }));
    }
    rows.sort_by_key(|r| {
        let mount = r["mount"].as_str().unwrap_or("");
        DISK_MOUNTS.iter().position(|m| *m == mount).unwrap_or(99)
    });
    rows
}

fn read_diskstats() -> HashMap<String, (i64, i64)> {
    let mut result = HashMap::new();
    let content = fs::read_to_string("/proc/diskstats").unwrap_or_default();
    for line in content.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 14 {
            continue;
        }
        let device = parts[2];
        if !IO_DEVICES.contains(&device) {
            continue;
        }
        if let (Ok(r), Ok(w)) = (parts[5].parse::<i64>(), parts[9].parse::<i64>()) {
            result.insert(device.to_string(), (r, w));
        }
    }
    result
}

fn read_loadavg() -> [f64; 3] {
    let content = fs::read_to_string("/proc/loadavg").unwrap_or_default();
    let parts: Vec<&str> = content.split_whitespace().collect();
    [
        parts.get(0).and_then(|s| s.parse().ok()).unwrap_or(0.0),
        parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0.0),
        parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0.0),
    ]
}

fn read_proc_count() -> usize {
    fs::read_dir("/proc")
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.file_name()
                        .to_str()
                        .map(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))
                        .unwrap_or(false)
                })
                .count()
        })
        .unwrap_or(0)
}

fn read_bluetooth() -> Value {
    if !Path::new("/sys/class/bluetooth/hci0").is_dir() {
        return json!({"present": false});
    }
    let out = run_capture("hciconfig", &["-a", "hci0"]);
    let up = out.contains("UP RUNNING");
    let name = out.find("Name: '").and_then(|start| {
        let rest = &out[start + "Name: '".len()..];
        rest.find('\'').map(|end| rest[..end].to_string())
    });
    json!({"present": true, "up": up, "name": name})
}

fn read_net_ifaces() -> Vec<Value> {
    let mut ifaces = Vec::new();
    let base = Path::new("/sys/class/net");
    if !base.is_dir() {
        return ifaces;
    }
    let mut names: Vec<String> = fs::read_dir(base)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter_map(|e| e.file_name().to_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    for name in names {
        if !NET_IFACES_OF_INTEREST.contains(&name.as_str()) {
            continue;
        }
        let state_path = base.join(&name).join("operstate");
        let state = fs::read_to_string(&state_path)
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| "unknown".to_string());
        ifaces.push(json!({"iface": name, "state": state}));
    }
    ifaces
}

fn push_history(history: &mut HashMap<String, Vec<f64>>, key: &str, value: f64) {
    let bucket = history.entry(key.to_string()).or_default();
    bucket.push(value);
    if bucket.len() > HISTORY_LEN {
        let excess = bucket.len() - HISTORY_LEN;
        bucket.drain(0..excess);
    }
}

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

fn main() {
    if let Some(parent) = Path::new(OUTPUT_PATH).parent() {
        let _ = fs::create_dir_all(parent);
    }

    let mut prev_cpu = read_proc_stat();
    let mut prev_disk_io = read_diskstats();
    let mut prev_time = now_secs();
    let mut history: HashMap<String, Vec<f64>> = HashMap::new();

    // First tick needs a baseline before any delta makes sense.
    std::thread::sleep(Duration::from_secs_f64(1.0));

    loop {
        let tick_start = now_secs();
        let now = tick_start;
        let dt = now - prev_time;

        let curr_cpu = read_proc_stat();
        let zero: CpuLine = [0; 8];
        let cpu_total_pct = cpu_pct(prev_cpu.get("cpu").unwrap_or(&zero), curr_cpu.get("cpu").unwrap_or(&zero));
        let mut per_core = Vec::new();
        let mut core_idx = 0usize;
        loop {
            let label = format!("cpu{core_idx}");
            let curr_line = match curr_cpu.get(&label) {
                Some(l) => l,
                None => break,
            };
            let prev_line = prev_cpu.get(&label).unwrap_or(&zero);
            per_core.push(round1(cpu_pct(prev_line, curr_line)));
            core_idx += 1;
        }
        prev_cpu = curr_cpu;

        let mem = read_meminfo();
        let disks = read_disk_usage();

        let curr_disk_io = read_diskstats();
        let mut io_read_kb_s = 0.0;
        let mut io_write_kb_s = 0.0;
        let mut io_obj = serde_json::Map::new();
        for device in IO_DEVICES {
            let (prev_r, prev_w) = *prev_disk_io.get(device).unwrap_or(&(0, 0));
            let (curr_r, curr_w) = *curr_disk_io.get(device).unwrap_or(&(0, 0));
            let read_kb_s = if dt > 0.0 {
                ((curr_r - prev_r) as f64 * 512.0 / 1024.0) / dt
            } else {
                0.0
            };
            let write_kb_s = if dt > 0.0 {
                ((curr_w - prev_w) as f64 * 512.0 / 1024.0) / dt
            } else {
                0.0
            };
            let read_kb_s = round1(read_kb_s.max(0.0));
            let write_kb_s = round1(write_kb_s.max(0.0));
            if device == "mmcblk0" {
                io_read_kb_s = read_kb_s;
                io_write_kb_s = write_kb_s;
            }
            io_obj.insert(
                device.to_string(),
                json!({"read_kb_s": read_kb_s, "write_kb_s": write_kb_s}),
            );
        }
        prev_disk_io = curr_disk_io;
        prev_time = now;

        let load = read_loadavg();
        let proc_count = read_proc_count();
        let bluetooth = read_bluetooth();
        let net = read_net_ifaces();

        push_history(&mut history, "cpu_total_pct", round1(cpu_total_pct));
        push_history(&mut history, "mem_pct", mem.pct);
        push_history(&mut history, "io_read_kb_s", io_read_kb_s);
        push_history(&mut history, "io_write_kb_s", io_write_kb_s);

        let snapshot = json!({
            "ts": (now * 1000.0) as i64,
            "interval_s": INTERVAL_S,
            "cpu": {
                "total_pct": round1(cpu_total_pct),
                "per_core": per_core,
                "history": history.get("cpu_total_pct").cloned().unwrap_or_default(),
            },
            "mem": {
                "total_kb": mem.total_kb,
                "used_kb": mem.used_kb,
                "available_kb": mem.available_kb,
                "cached_kb": mem.cached_kb,
                "buffers_kb": mem.buffers_kb,
                "pct": mem.pct,
                "history": history.get("mem_pct").cloned().unwrap_or_default(),
            },
            "disk": disks,
            "io": Value::Object(io_obj),
            "io_history": {
                "read_kb_s": history.get("io_read_kb_s").cloned().unwrap_or_default(),
                "write_kb_s": history.get("io_write_kb_s").cloned().unwrap_or_default(),
            },
            "load": load,
            "proc_count": proc_count,
            "bluetooth": bluetooth,
            "net": net,
        });

        let tmp_path = format!("{OUTPUT_PATH}.tmp");
        if let Err(e) = fs::write(&tmp_path, snapshot.to_string()) {
            eprintln!("[noc-agent] failed to write {tmp_path}: {e}");
        } else if let Err(e) = fs::rename(&tmp_path, OUTPUT_PATH) {
            eprintln!("[noc-agent] failed to rename into {OUTPUT_PATH}: {e}");
        }

        let elapsed = now_secs() - tick_start;
        let sleep_s = (INTERVAL_S - elapsed).max(0.1);
        std::thread::sleep(Duration::from_secs_f64(sleep_s));
    }
}
