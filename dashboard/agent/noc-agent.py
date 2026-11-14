#!/usr/bin/env python3
"""On-device metrics collector for the NOC dashboard.

Runs on the Car Thing itself (python3 is present in the stock Buildroot
image, see docs/dashboard.md for the discovery that led to picking Python
over a BusyBox ash script). Every INTERVAL seconds it reads /proc/stat,
/proc/meminfo, /proc/diskstats, /proc/loadavg, `df`, and bluetooth/net
sysfs state, computes deltas where needed (CPU%, disk I/O rate), and
writes the result as a single JSON file that the static frontend polls
over plain HTTP. No daemon dependencies beyond the Python 3.7 stdlib
already on the device.
"""

import json
import os
import re
import subprocess
import time

INTERVAL = 2.0
OUTPUT_PATH = "/tmp/noc-dashboard/metrics.json"
HISTORY_LEN = 60  # ~2 minutes of history at the default interval

DISK_MOUNTS = ["/", "/var", "/var/lib", "/home"]
IO_DEVICES = ["mmcblk0"]
NET_IFACES_OF_INTEREST = ["bnep0", "usb0", "eth0", "wlan0"]


def read_proc_stat():
    """Return {label: (user, nice, system, idle, iowait, irq, softirq, steal)} per cpu line."""
    cpus = {}
    with open("/proc/stat") as f:
        for line in f:
            if not line.startswith("cpu"):
                continue
            parts = line.split()
            label = parts[0]
            nums = [int(x) for x in parts[1:9]]
            cpus[label] = nums
    return cpus


def cpu_pct(prev, curr):
    """Percent busy between two /proc/stat samples for one cpu line."""
    prev_idle = prev[3] + prev[4]
    curr_idle = curr[3] + curr[4]
    prev_total = sum(prev)
    curr_total = sum(curr)
    total_delta = curr_total - prev_total
    idle_delta = curr_idle - prev_idle
    if total_delta <= 0:
        return 0.0
    return max(0.0, min(100.0, 100.0 * (total_delta - idle_delta) / total_delta))


def read_meminfo():
    info = {}
    with open("/proc/meminfo") as f:
        for line in f:
            m = re.match(r"(\w+):\s+(\d+)\s*kB", line)
            if m:
                info[m.group(1)] = int(m.group(2))
    total = info.get("MemTotal", 0)
    available = info.get("MemAvailable", 0)
    used = total - info.get("MemFree", 0) - info.get("Buffers", 0) - info.get("Cached", 0)
    return {
        "total_kb": total,
        "used_kb": used,
        "available_kb": available,
        "cached_kb": info.get("Cached", 0),
        "buffers_kb": info.get("Buffers", 0),
        "pct": round(100.0 * used / total, 1) if total else 0.0,
    }


def read_disk_usage():
    out = subprocess.run(["df", "-k"], capture_output=True, text=True, check=False).stdout
    rows = []
    seen = set()
    for line in out.splitlines()[1:]:
        parts = line.split()
        if len(parts) < 6:
            continue
        filesystem, size_kb, used_kb, avail_kb, pct, mount = parts[0], parts[1], parts[2], parts[3], parts[4], parts[5]
        if mount not in DISK_MOUNTS:
            continue
        key = (filesystem, mount)
        if key in seen:
            continue
        seen.add(key)
        try:
            size_kb_i, used_kb_i, avail_kb_i = int(size_kb), int(used_kb), int(avail_kb)
        except ValueError:
            continue
        pct_i = int(pct.rstrip("%")) if pct.rstrip("%").isdigit() else 0
        rows.append({
            "mount": mount,
            "filesystem": filesystem,
            "size_kb": size_kb_i,
            "used_kb": used_kb_i,
            "avail_kb": avail_kb_i,
            "pct": pct_i,
        })
    rows.sort(key=lambda r: DISK_MOUNTS.index(r["mount"]) if r["mount"] in DISK_MOUNTS else 99)
    return rows


def read_diskstats():
    """Return {device: (read_sectors, write_sectors)} for devices of interest."""
    result = {}
    with open("/proc/diskstats") as f:
        for line in f:
            parts = line.split()
            if len(parts) < 14:
                continue
            device = parts[2]
            if device not in IO_DEVICES:
                continue
            read_sectors = int(parts[5])
            write_sectors = int(parts[9])
            result[device] = (read_sectors, write_sectors)
    return result


def read_loadavg():
    with open("/proc/loadavg") as f:
        parts = f.read().split()
    return [float(parts[0]), float(parts[1]), float(parts[2])]


def read_proc_count():
    return len(os.listdir("/proc")) and sum(1 for name in os.listdir("/proc") if name.isdigit())


def read_bluetooth():
    hci_path = "/sys/class/bluetooth/hci0"
    if not os.path.isdir(hci_path):
        return {"present": False}
    out = subprocess.run(["hciconfig", "-a", "hci0"], capture_output=True, text=True, check=False).stdout
    up = "UP RUNNING" in out
    name_match = re.search(r"Name: '([^']*)'", out)
    return {
        "present": True,
        "up": up,
        "name": name_match.group(1) if name_match else None,
    }


def read_net_ifaces():
    ifaces = []
    base = "/sys/class/net"
    if not os.path.isdir(base):
        return ifaces
    for name in sorted(os.listdir(base)):
        if name not in NET_IFACES_OF_INTEREST:
            continue
        state_path = os.path.join(base, name, "operstate")
        state = "unknown"
        if os.path.exists(state_path):
            with open(state_path) as f:
                state = f.read().strip()
        ifaces.append({"iface": name, "state": state})
    return ifaces


def push_history(history, key, value):
    bucket = history.setdefault(key, [])
    bucket.append(value)
    if len(bucket) > HISTORY_LEN:
        del bucket[0: len(bucket) - HISTORY_LEN]


def main():
    os.makedirs(os.path.dirname(OUTPUT_PATH), exist_ok=True)

    prev_cpu = read_proc_stat()
    prev_disk_io = read_diskstats()
    prev_time = time.time()
    history = {}

    # first tick needs a baseline before any delta makes sense
    time.sleep(1.0)

    while True:
        now = time.time()
        dt = now - prev_time

        curr_cpu = read_proc_stat()
        cpu_total_pct = cpu_pct(prev_cpu.get("cpu", [0] * 8), curr_cpu.get("cpu", [0] * 8))
        per_core = []
        core_idx = 0
        while True:
            label = f"cpu{core_idx}"
            if label not in curr_cpu:
                break
            per_core.append(round(cpu_pct(prev_cpu.get(label, [0] * 8), curr_cpu[label]), 1))
            core_idx += 1
        prev_cpu = curr_cpu

        mem = read_meminfo()
        disks = read_disk_usage()

        curr_disk_io = read_diskstats()
        io = {}
        for device in IO_DEVICES:
            prev_r, prev_w = prev_disk_io.get(device, (0, 0))
            curr_r, curr_w = curr_disk_io.get(device, (0, 0))
            read_kb_s = ((curr_r - prev_r) * 512 / 1024) / dt if dt > 0 else 0.0
            write_kb_s = ((curr_w - prev_w) * 512 / 1024) / dt if dt > 0 else 0.0
            io[device] = {
                "read_kb_s": round(max(0.0, read_kb_s), 1),
                "write_kb_s": round(max(0.0, write_kb_s), 1),
            }
        prev_disk_io = curr_disk_io
        prev_time = now

        load = read_loadavg()
        proc_count = read_proc_count()
        bluetooth = read_bluetooth()
        net = read_net_ifaces()

        push_history(history, "cpu_total_pct", round(cpu_total_pct, 1))
        push_history(history, "mem_pct", mem["pct"])
        push_history(history, "io_read_kb_s", io.get("mmcblk0", {}).get("read_kb_s", 0.0))
        push_history(history, "io_write_kb_s", io.get("mmcblk0", {}).get("write_kb_s", 0.0))

        snapshot = {
            "ts": int(now * 1000),
            "interval_s": INTERVAL,
            "cpu": {
                "total_pct": round(cpu_total_pct, 1),
                "per_core": per_core,
                "history": history.get("cpu_total_pct", []),
            },
            "mem": {
                **mem,
                "history": history.get("mem_pct", []),
            },
            "disk": disks,
            "io": io,
            "io_history": {
                "read_kb_s": history.get("io_read_kb_s", []),
                "write_kb_s": history.get("io_write_kb_s", []),
            },
            "load": load,
            "proc_count": proc_count,
            "bluetooth": bluetooth,
            "net": net,
        }

        tmp_path = OUTPUT_PATH + ".tmp"
        with open(tmp_path, "w") as f:
            json.dump(snapshot, f)
        os.rename(tmp_path, OUTPUT_PATH)

        time.sleep(max(0.1, INTERVAL - (time.time() - now)))


if __name__ == "__main__":
    main()
