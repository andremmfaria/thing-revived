# NOC Dashboard

This is a standalone deliverable, separate from the hardware-input bridge/UI prototype documented in [`getting-started.md`](getting-started.md). It shows the Car Thing's own internals, CPU, memory, disk/eMMC usage, storage I/O, Bluetooth adapter state, and network interface state, rendered as a dark, high-contrast monitoring wall in the style of a datacenter NOC (Network Operations Center) display: circular gauges for utilization percentages, rolling sparkline history strips, a grid of tiles, a monospace font, and glowing accent lines on a near-black background. All data shown is read live off the physical device over `adb shell`, there is no fabricated or placeholder data in the shipped code path.

## What it shows

- **CPU**: overall busy percentage as a circular gauge, per-core percentages as a row of small bars, and a sparkline of the last 60 samples (about 2 minutes at the default 2 second poll interval). Computed from two `/proc/stat` samples a tick apart, the standard `(total_delta - idle_delta) / total_delta` method.
- **Memory**: used percentage as a circular gauge, with available/cached/buffers broken out underneath in megabytes, and a sparkline of the used percentage history. Read from `/proc/meminfo`.
- **Disk / eMMC**: a bar per mounted filesystem (`/`, `/var`, `/var/lib`, `/home`, matching the partitions documented in [`research/hardware-verification.md`](research/hardware-verification.md)) showing used/total in megabytes and percent used. Read from `df -k`.
- **I/O**: read and write throughput in KB/s for the `mmcblk0` block device, plus a sparkline of recent history. Computed from `/proc/diskstats` sector-count deltas (512 bytes per sector) between ticks.
- **Load / process count**: 1/5/15 minute load averages from `/proc/loadavg`, and a live count of entries under `/proc` whose name is numeric (one per running process).
- **Bluetooth**: `hci0` up/down state and the adapter's advertised name, from `hciconfig -a hci0`.
- **Network**: operstate (`up`/`down`) for `bnep0`, `usb0`, `eth0`, and `wlan0` if present, read from `/sys/class/net/<iface>/operstate`. On the unit used for this work none of these interfaces exist right now (only loopback and tunnel pseudo-interfaces are present, see the verification section below), so this tile legitimately shows "no active interfaces" rather than being broken.

## Architecture

The design deliberately avoids adding any new cross-compiled binary to the device. The existing `bridge/` prototype in this repo is a Rust `armv7-unknown-linux-gnueabihf` binary because it needs `evdev` access; this dashboard only needs to read `/proc`, run `df`, and run `hciconfig`, all of which a plain script on the device can already do without a build step.

A quick on-device capability check (`adb shell "which free df top awk bc python3 ps hciconfig"`) showed Python 3.7.7 is already installed on the device alongside the usual BusyBox utilities. Python was chosen over a BusyBox `ash` script for this piece specifically because the CPU and I/O percentage math needs persistent state between ticks (the previous `/proc/stat` and `/proc/diskstats` samples) and a bit of parsing, both of which are straightforward in Python and comparatively fiddly in `ash`. This keeps the agent to one dependency-free script using only the Python 3.7 standard library.

- `dashboard/agent/noc-agent.py`: runs on the device. Every 2 seconds it reads `/proc/stat`, `/proc/meminfo`, `/proc/diskstats`, `/proc/loadavg`, runs `df -k` and `hciconfig -a hci0`, computes the CPU and I/O deltas against the previous tick, keeps a rolling 60-sample history per metric in memory, and atomically writes the whole snapshot as one JSON file to `/tmp/noc-dashboard/metrics.json` (write to a `.tmp` file then `os.rename`, so a reader never sees a half-written file).
- `dashboard/frontend/`: a static `index.html` + `style.css` + `app.js` page with no build step and no framework. It polls `metrics.json` over plain HTTP every 2 seconds with `fetch`, and draws the gauges (SVG circles with `stroke-dashoffset`), sparklines (`<canvas>` line charts), and bars with vanilla DOM updates. The JavaScript intentionally avoids ES2020 syntax such as optional chaining (`?.`) and nullish coalescing (`??`): the sibling `ui/` prototype already hit the problem that Chromium 69, the version actually running on this device (see [`hardware.md`](hardware.md)), silently fails to parse that syntax with no console error and the page just never mounts. Everything here is plain ES6 (`const`/`let`, arrow functions, template literals, `async`/`await`, `fetch`), which Chromium 69 runs fine.
- Both pieces are served from the same directory, `/tmp/noc-dashboard/` on the device, by a plain `python3 -m http.server`, the same approach already used for the `ui/` prototype in [`getting-started.md`](getting-started.md). This was simpler than standing up a WebSocket endpoint for a metrics feed that only needs to update every couple of seconds; a static JSON file polled over HTTP is enough and keeps the on-device process count at two (the agent and the HTTP server) with no new third-party dependency.

## Persistent deployment (survives reboot, no manual steps after setup)

This is the primary deployment path, parallel to the one documented in [`getting-started.md`](getting-started.md) for the main control-surface UI, but kept entirely separate from it. The dashboard lives at its own on-device path, is not the default boot screen, and is reached on demand through the same CDP `Page.navigate` trick, with no `adb push` or manual process start needed first.

A real on-device constraint drove the exact paths used here: the root filesystem (which carries `/usr/share` and `/opt`) remounts read-only on every boot by default. A plain `mount -o remount,rw /` only lasts until the next reboot, it does not persist. That is fine for static content that is only read, like the frontend files or the bridge binary, but it breaks anything that needs to keep writing at runtime, like the metrics file, if that file lives under `/usr/share` or `/opt`. `/var`, `/var/lib`, and `/home` are separate ext4 mounts that stay `rw` across reboots with no remount needed, confirmed with a real `adb reboot` followed by a write test against each path.

Given that, the pieces land as follows:

- `dashboard/agent/noc-agent.py`: pushed to `/opt/thing-revived/noc-agent.py`, the same persistent-but-read-only-after-boot convention already used for the bridge daemon. The script itself is static once pushed, so living on the read-only-after-reboot root fs is fine.
- The agent's live output, `noc-metrics.json`, is written to `/var/thing-revived/noc-metrics.json`, a separate writable mount, not `/tmp` (tmpfs, wiped every reboot) and not `/usr/share` or `/opt` (read-only after reboot).
- `dashboard/frontend/` (`index.html`, `style.css`, `app.js`) is pushed to `/usr/share/thing-revived-dashboard/`, a separate directory from `/usr/share/qt-superbird-app/webapp/` (the main UI's path, untouched by this deliverable).

### 1. Push the agent and add its supervisord entry

```sh
adb shell "mount -o remount,rw /"
adb shell "mkdir -p /opt/thing-revived /var/thing-revived"
adb push dashboard/agent/noc-agent.py /opt/thing-revived/noc-agent.py
```

Add a new supervisord program definition as its own file under `/etc/supervisor.d/`, picked up automatically via the existing `[include] files = /etc/supervisor.d/*.conf` directive, the same pattern `thing-revived-bridge.conf` already uses:

```ini
[program:thing-revived-noc-agent]
command=/usr/bin/python3 /opt/thing-revived/noc-agent.py
numprocs=1
autorestart=true
autostart=true
startsecs=0
priority=96
redirect_stderr=true
stdout_logfile=/var/log/thing-revived-noc-agent.info
stdout_logfile_maxbytes=1MB
stdout_logfile_backups=3
```

```sh
adb push thing-revived-noc-agent.conf /etc/supervisor.d/thing-revived-noc-agent.conf
adb shell "supervisorctl reread && supervisorctl update"
```

### 2. Push the frontend to its own permanent path

```sh
adb push dashboard/frontend/index.html dashboard/frontend/style.css dashboard/frontend/app.js /usr/share/thing-revived-dashboard/
```

This never touches `/usr/share/qt-superbird-app/webapp/`, which remains the main UI's path and the device's default boot screen.

### 3. fetch() vs XMLHttpRequest on file://, verified on the real device

The frontend polls the metrics file from a page loaded over `file://`, since there is no server process here by design, same reasoning as the main UI's persistence setup. This was tested directly against the on-device Chromium 69 over CDP, not assumed:

- `fetch('metrics.json')` on a `file://` page does retrieve the file body (confirmed: `.text()` returned the real content), but the `Response.status` is always `0` for `file://` URLs. That makes `Response.ok` permanently `false`, so the original `app.js` code (`if (!res.ok) throw ...`) would reject on every single poll and the dashboard would be stuck showing "NO SIGNAL" forever even though the data was actually arriving.
- Switching to `XMLHttpRequest` (which the existing chromium supervisord entry's `--allow-file-access-from-files` flag already supports) and judging success on getting parseable JSON back rather than on `xhr.status`, works correctly. Confirmed over CDP `Runtime.evaluate`: a synchronous `XMLHttpRequest` against the metrics file returned `status=0` but a full, correct `responseText`.
- `app.js` was changed to use `XMLHttpRequest` for this reason. No local HTTP server was needed, the fallback plan in this doc's original draft (`python3 -m http.server`, supervisord-managed) was not required.

### 4. Metrics file location and the frontend's absolute file:// reference

`noc-agent.py`'s `OUTPUT_PATH` is `/var/thing-revived/noc-metrics.json` (not `/tmp`, not `/usr/share`, see the filesystem-writability note above). Because the frontend page lives in a different directory tree (`/usr/share/thing-revived-dashboard/`), `app.js`'s `METRICS_URL` is an absolute `file:///var/thing-revived/noc-metrics.json`, not a relative path. `--allow-file-access-from-files` permits this cross-directory `file://` to `file://` read; this was exercised for real over CDP, not assumed.

### 5. Viewing it on demand, no adb push or manual server start needed

```sh
adb forward tcp:2222 tcp:2222
curl localhost:2222/json   # find the active page's target id
# Page.navigate that target's websocket to file:///usr/share/thing-revived-dashboard/index.html
```

Navigate back to `file:///usr/share/qt-superbird-app/webapp/index.html` afterwards to leave the device on the main UI, which stays the default boot screen; the dashboard is reached on demand only.

### What was actually verified on the real device, with a real reboot

- Pushed `noc-agent.py` to `/opt/thing-revived/`, the frontend to `/usr/share/thing-revived-dashboard/`, and the new `thing-revived-noc-agent.conf` to `/etc/supervisor.d/`, then ran `supervisorctl reread && supervisorctl update`; `supervisorctl status` showed `thing-revived-noc-agent` as `RUNNING` alongside the existing `thing-revived-bridge` and `chromium` entries.
- First attempt at the writable-path question actually failed in testing: pointing `OUTPUT_PATH` at `/usr/share/thing-revived-dashboard/metrics.json` worked fine until a real `adb reboot`, after which the agent's log (`/var/log/thing-revived-noc-agent.info`) filled with `OSError: [Errno 30] Read-only file system`, because the earlier `mount -o remount,rw /` had not survived the reboot. This is what led to moving the output to `/var/thing-revived/noc-metrics.json` instead, confirmed writable across a real reboot afterward.
- Ran a real `adb reboot`, waited for the device to reappear in `adb devices`, then ran `supervisorctl status` with no manual steps: both `thing-revived-noc-agent` and `thing-revived-bridge` showed `RUNNING`.
- Read `/var/thing-revived/noc-metrics.json`'s `ts` field twice, 4 seconds apart, immediately after that reboot: the value advanced by about 4000ms, matching the agent's 2 second poll interval, confirming the metrics file is being actively and continuously written purely by the supervisord-managed process, with nobody manually starting anything.
- Forwarded CDP port 2222, confirmed the active page target was still on the main UI (`file:///usr/share/qt-superbird-app/webapp/index.html`, the default boot target, untouched by this work), then sent `Page.navigate` to `file:///usr/share/thing-revived-dashboard/index.html` and `Page.captureScreenshot`. The screenshot showed the dashboard fully rendered with real live data, CPU and memory gauges, disk bars, I/O sparkline, load averages, Bluetooth status, and a green "LIVE" indicator, all matching the device's actual state.
- Navigated the CDP target back to `file:///usr/share/qt-superbird-app/webapp/index.html` afterward, confirmed via `curl localhost:2222/json`, so the device was left showing the main UI, not the dashboard.

## Quick iteration during development (temporary, does not survive reboot)

For fast edit-test loops while developing the frontend or agent without touching the persistent deployment above, the original manual-serve approach is still useful. Push the agent script and the static frontend files to the device, then start the collector and the HTTP server:

```sh
adb -s <serial> shell "mkdir -p /tmp/noc-dashboard"
adb -s <serial> push dashboard/agent/noc-agent.py /tmp/noc-dashboard/noc-agent.py
adb -s <serial> push dashboard/frontend/index.html dashboard/frontend/style.css dashboard/frontend/app.js /tmp/noc-dashboard/
```

Starting the two on-device processes needs a persistent `adb shell` connection. On this device, backgrounding a command inside a single `adb shell "... &"` call does not survive the call returning, the device kills everything attached to that shell session once it closes, even with `nohup` or `setsid`. What does work is keeping the `adb shell` command itself running as a long-lived background job on the host machine, so the connection to the device is never closed:

```sh
adb -s <serial> shell "cd /tmp/noc-dashboard && python3 noc-agent.py" &
adb -s <serial> shell "cd /tmp/noc-dashboard && python3 -m http.server 8081 --bind 127.0.0.1" &
```

Then either view it on the device's own screen, using the same CDP `Page.navigate` procedure already proven in [`getting-started.md`](getting-started.md):

```sh
adb -s <serial> forward tcp:2222 tcp:2222
curl localhost:2222/json   # find the active page's target id
# Page.navigate that target's websocket to http://127.0.0.1:8081/index.html
```

or point a browser on the same network at `http://<device-ip>:8081/` once the HTTP server is bound to a reachable interface instead of `127.0.0.1` (change the `--bind` address to `0.0.0.0` for that case). See the verification section below for which of these two paths was actually exercised against the real unit.

## What was actually verified on the real device (manual-serve path, dev iteration)

This was run against the physical unit, serial `8559R486QR1S`, not simulated or faked. This section covers the temporary manual-serve path above; see "What was actually verified on the real device, with a real reboot" earlier in this document for the persistent deployment's own verification.

- Confirmed `free`, `df`, `top`, `awk`, `bc`, `python3` (3.7.7), `ps`, and `hciconfig` are all present on-device before choosing the implementation approach.
- Ran `noc-agent.py` on the device for several minutes. `/tmp/noc-dashboard/metrics.json` updated every 2 seconds with real, varying numbers, for example CPU total around 15 to 57 percent across different samples, memory used around 35 percent of 499928 kB total, disk usage 63 percent on `/` and 10 percent on `/var` and `/home` (matching `df`'s own output run directly over `adb shell` as a cross-check), and `hci0` reporting up with the adapter name `Car Thing (SN: QR1S)`.
- Confirmed over `adb forward tcp:8081 tcp:8081` plus `curl http://127.0.0.1:8081/metrics.json` that the HTTP server serves the live JSON file correctly from the host side of the USB connection, this is the same thing a browser on the same network would receive if pointed at the device's IP on port 8081, but no browser-on-LAN check was actually run, because this unit currently has no `usb0`/`bnep0`/other real network interface up, `ip addr` only shows loopback and unused tunnel interfaces. That path is implemented and should work once the device has a reachable network interface, but it is not yet verified end-to-end; only the adb-forwarded HTTP path was.
- Confirmed on the device's own physical screen: forwarded the existing CDP remote-debugging port 2222, found the active page target, sent a `Page.navigate` command to `http://127.0.0.1:8081/index.html`, waited for load, and took a `Page.captureScreenshot`. The resulting screenshot showed the dashboard fully rendered on-device: the CPU gauge at 57 percent with a green ring and an upward-trending sparkline, the memory gauge at 35 percent with the avail/cache/buff breakdown, four disk bars matching the real `df` numbers, I/O read/write numbers with a cyan sparkline, load averages and a process count of 161, a green "UP" Bluetooth status with the adapter's real name, the live clock ticking, and the network tile correctly reporting no active interfaces. This confirms the page actually works on the real Chromium 69 engine running on the device, not just in a desktop browser.
- A desktop-browser check of the same forwarded URL was attempted as a second confirmation but failed for an unrelated environment reason, a local Chromium profile lock permission error in the automation tool's own cache directory, not a problem with the dashboard. The on-device CDP screenshot above is the verified visual proof for this deliverable.

## Known limitations

- I/O is read-only throughput for the whole `mmcblk0` device, not broken out per-partition. `/proc/diskstats` does expose per-partition numbers (`mmcblk0p1` through `mmcblk0p18`), but the device-level `mmcblk0` line is the only one that saw non-zero read/write activity during testing, so this was kept simple rather than adding four empty per-partition sparklines.
- Network interface state only covers `bnep0`, `usb0`, `eth0`, and `wlan0`. None of those exist on this unit right now, so that tile is currently a placeholder in practice until a Bluetooth PAN or USB network link is brought up, which is tracked separately in [`architecture.md`](architecture.md).
- In the manual-serve dev path above, the agent is started by hand over `adb shell` and is not supervised; if it or the HTTP server dies, nothing restarts it automatically. The persistent deployment path earlier in this document resolves this: both the agent and (if ever needed) a local server are supervisord-managed with `autostart=true` and `autorestart=true`, confirmed to come back up on their own after a real reboot.
