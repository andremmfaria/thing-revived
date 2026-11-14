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

## Running it

Push the agent script and the static frontend files to the device, then start the collector and the HTTP server:

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

## What was actually verified on the real device

This was run against the physical unit, serial `8559R486QR1S`, not simulated or faked.

- Confirmed `free`, `df`, `top`, `awk`, `bc`, `python3` (3.7.7), `ps`, and `hciconfig` are all present on-device before choosing the implementation approach.
- Ran `noc-agent.py` on the device for several minutes. `/tmp/noc-dashboard/metrics.json` updated every 2 seconds with real, varying numbers, for example CPU total around 15 to 57 percent across different samples, memory used around 35 percent of 499928 kB total, disk usage 63 percent on `/` and 10 percent on `/var` and `/home` (matching `df`'s own output run directly over `adb shell` as a cross-check), and `hci0` reporting up with the adapter name `Car Thing (SN: QR1S)`.
- Confirmed over `adb forward tcp:8081 tcp:8081` plus `curl http://127.0.0.1:8081/metrics.json` that the HTTP server serves the live JSON file correctly from the host side of the USB connection, this is the same thing a browser on the same network would receive if pointed at the device's IP on port 8081, but no browser-on-LAN check was actually run, because this unit currently has no `usb0`/`bnep0`/other real network interface up, `ip addr` only shows loopback and unused tunnel interfaces. That path is implemented and should work once the device has a reachable network interface, but it is not yet verified end-to-end; only the adb-forwarded HTTP path was.
- Confirmed on the device's own physical screen: forwarded the existing CDP remote-debugging port 2222, found the active page target, sent a `Page.navigate` command to `http://127.0.0.1:8081/index.html`, waited for load, and took a `Page.captureScreenshot`. The resulting screenshot showed the dashboard fully rendered on-device: the CPU gauge at 57 percent with a green ring and an upward-trending sparkline, the memory gauge at 35 percent with the avail/cache/buff breakdown, four disk bars matching the real `df` numbers, I/O read/write numbers with a cyan sparkline, load averages and a process count of 161, a green "UP" Bluetooth status with the adapter's real name, the live clock ticking, and the network tile correctly reporting no active interfaces. This confirms the page actually works on the real Chromium 69 engine running on the device, not just in a desktop browser.
- A desktop-browser check of the same forwarded URL was attempted as a second confirmation but failed for an unrelated environment reason, a local Chromium profile lock permission error in the automation tool's own cache directory, not a problem with the dashboard. The on-device CDP screenshot above is the verified visual proof for this deliverable.

## Known limitations

- I/O is read-only throughput for the whole `mmcblk0` device, not broken out per-partition. `/proc/diskstats` does expose per-partition numbers (`mmcblk0p1` through `mmcblk0p18`), but the device-level `mmcblk0` line is the only one that saw non-zero read/write activity during testing, so this was kept simple rather than adding four empty per-partition sparklines.
- Network interface state only covers `bnep0`, `usb0`, `eth0`, and `wlan0`. None of those exist on this unit right now, so that tile is currently a placeholder in practice until a Bluetooth PAN or USB network link is brought up, which is tracked separately in [`architecture.md`](architecture.md).
- The agent is started by hand over `adb shell` and is not supervised, if it or the HTTP server dies, nothing restarts it automatically. That matches the level of process supervision the `bridge`/`ui` prototype also has today; adding a proper init.d entry or watchdog is future work, not part of this deliverable.
