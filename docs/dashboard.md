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

This agent originally shipped as a Python 3.7 stdlib script (`noc-agent.py`), chosen at the time specifically to avoid adding a second cross-compiled binary alongside `prototype/control-surface/agent/`'s `armv7-unknown-linux-gnueabihf` build: it only needs to read `/proc`, run `df`, and run `hciconfig`, all doable from a plain on-device script with no build step, and a capability check (`adb shell "which free df top awk bc python3 ps hciconfig"`) had confirmed Python 3.7.7 was already present on the device.

**It has since been rewritten in Rust** (`prototype/dashboard/agent/`, binary `noc-agent`) and the Python source deleted; **no Python remains anywhere in this repo.** The rewrite now pays the same `armv7-unknown-linux-gnueabihf` / `+crt-static` cross-compile cost `prototype/control-surface/agent/` already pays, following the exact same build convention (see [`getting-started.md`](getting-started.md#3-build-and-deploy-the-bridge-daemon-with-a-supervisord-entry-so-it-survives-reboot)). The CPU/I/O delta math that justified picking a language with easy persistent state over BusyBox `ash` in the first place (the previous `/proc/stat` and `/proc/diskstats` samples, carried between ticks) is just as straightforward in Rust (`loop { ... }` with local `let mut` state) as it was in Python, so there was no remaining reason to keep a second language on the device purely for this one agent. The only dependency is `serde_json`, used to build and serialize the exact same JSON shape the Python version produced.

- `prototype/dashboard/agent/` (Rust binary `noc-agent`): runs on the device. Every 2 seconds it reads `/proc/stat`, `/proc/meminfo`, `/proc/diskstats`, `/proc/loadavg`, runs `df -k` and `hciconfig -a hci0` as subprocesses, computes the CPU and I/O deltas against the previous tick, keeps a rolling 60-sample history per metric in memory, and atomically writes the whole snapshot as one JSON file to `/var/thing-revived/noc-metrics.json` (write to a `.tmp` file then rename, so a reader never sees a half-written file) — field names and shapes are unchanged from the Python original, verified field-by-field (see "What changed in the Rust rewrite" below).
- `prototype/dashboard/frontend/`: a static `index.html` + `style.css` + `app.js` page with no build step and no framework, **unmodified by the Rust rewrite**. It polls `metrics.json` every 2 seconds and draws the gauges (SVG circles with `stroke-dashoffset`), sparklines (`<canvas>` line charts), and bars with vanilla DOM updates. The JavaScript intentionally avoids ES2020 syntax such as optional chaining (`?.`) and nullish coalescing (`??`): the sibling `prototype/control-surface/frontend/` prototype already hit the problem that Chromium 69, the version actually running on this device (see [`hardware.md`](hardware.md)), silently fails to parse that syntax with no console error and the page just never mounts. Everything here is plain ES6 (`const`/`let`, arrow functions, template literals, `async`/`await`), which Chromium 69 runs fine.
- In the persistent deployment path below, the frontend reads the metrics file directly over `file://` (via `XMLHttpRequest`, see §3 below); the "quick iteration" dev path further down still uses a local HTTP server for convenience, now started by hand rather than via `python3 -m http.server` (see that section for the current command).

### What changed in the Rust rewrite

- **Wire shape unchanged.** Every top-level and nested field name, type, and the 60-sample rolling-history behavior are identical to the Python version's output; `prototype/dashboard/frontend/app.js` required zero changes. This was checked field-by-field against the Python source during the port (`ts`, `interval_s`, `cpu.{total_pct,per_core,history}`, `mem.{total_kb,used_kb,available_kb,cached_kb,buffers_kb,pct,history}`, `disk[].{mount,filesystem,size_kb,used_kb,avail_kb,pct}`, `io.<device>.{read_kb_s,write_kb_s}`, `io_history.{read_kb_s,write_kb_s}`, `load`, `proc_count`, `bluetooth.{present,up,name}`, `net[].{iface,state}`), and confirmed live on a host run (see "What was actually verified" below).
- **Atomic write**: `fs::write` to `.tmp` then `fs::rename`, the same write-then-rename pattern as Python's `open()`/`os.rename()`.
- **No new runtime dependency on-device**: the Python version depended on the device's own Python 3.7 interpreter being present; the Rust version is a single statically-linked binary with no runtime dependency beyond the kernel ABI, same as `prototype/control-surface/agent/`.

## Persistent deployment (survives reboot, no manual steps after setup)

This is the primary deployment path, parallel to the one documented in [`getting-started.md`](getting-started.md) for the bridge/button-test UI, but kept entirely separate from it. As of this writing, the dashboard occupies the device's default boot path (`/usr/share/qt-superbird-app/webapp/`, what Chromium's `supervisord` entry always auto-loads on boot) and shows automatically on power-up, with the bridge/button-test UI moved to its own separate path instead. Which app is the default is a deliberate, swappable choice described in [`getting-started.md`](getting-started.md#swapping-which-app-is-the-default), not a fixed property of either project; keep this doc and that one in sync with whichever arrangement is actually live on the device.

A real on-device constraint drove the exact paths used here: the root filesystem (which carries `/usr/share` and `/opt`) remounts read-only on every boot by default. A plain `mount -o remount,rw /` only lasts until the next reboot, it does not persist. That is fine for static content that is only read, like the frontend files or the bridge binary, but it breaks anything that needs to keep writing at runtime, like the metrics file, if that file lives under `/usr/share` or `/opt`. `/var`, `/var/lib`, and `/home` are separate ext4 mounts that stay `rw` across reboots with no remount needed, confirmed with a real `adb reboot` followed by a write test against each path.

Given that, the pieces land as follows:

- `prototype/dashboard/agent/` (Rust binary `noc-agent`): pushed to `/opt/thing-revived/noc-agent`, the same persistent-but-read-only-after-boot convention already used for the bridge daemon. The binary is static once pushed, so living on the read-only-after-reboot root fs is fine.
- The agent's live output, `noc-metrics.json`, is written to `/var/thing-revived/noc-metrics.json`, a separate writable mount, not `/tmp` (tmpfs, wiped every reboot) and not `/usr/share` or `/opt` (read-only after reboot).
- `prototype/dashboard/frontend/` (`index.html`, `style.css`, `app.js`) is pushed directly to `/usr/share/qt-superbird-app/webapp/`, the path Chromium's `supervisord` entry always loads on boot, which is why the dashboard now shows automatically. The bridge/button-test UI's build lives at `/usr/share/thing-revived-uidemo/` instead; see [`getting-started.md`](getting-started.md).

### 1. Build, then push the agent binary and add its supervisord entry

```sh
cd prototype/dashboard/agent
RUSTFLAGS="-C target-feature=+crt-static" cargo build --release --target armv7-unknown-linux-gnueabihf
adb shell "mount -o remount,rw /"
adb shell "mkdir -p /opt/thing-revived /var/thing-revived"
adb push target/armv7-unknown-linux-gnueabihf/release/noc-agent /opt/thing-revived/noc-agent
adb shell "chmod +x /opt/thing-revived/noc-agent"
```

Same `armv7-unknown-linux-gnueabihf` target and `+crt-static` static-linking flag as `prototype/control-surface/agent/` (see [`getting-started.md`](getting-started.md) and [`hardware.md`](hardware.md)): static linking avoids a glibc-version mismatch between the cross-toolchain and the device's older on-device glibc.

Add a new supervisord program definition as its own file under `/etc/supervisor.d/`, picked up automatically via the existing `[include] files = /etc/supervisor.d/*.conf` directive, the same pattern `thing-revived-bridge.conf` already uses. Source kept at [`prototype/dashboard/deploy/supervisor.d/thing-revived-noc-agent.conf`](../prototype/dashboard/deploy/supervisor.d/thing-revived-noc-agent.conf):

```ini
[program:thing-revived-noc-agent]
command=/opt/thing-revived/noc-agent
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
adb push prototype/dashboard/deploy/supervisor.d/thing-revived-noc-agent.conf /etc/supervisor.d/thing-revived-noc-agent.conf
adb shell "supervisorctl reread && supervisorctl update"
```

### 2. Push the frontend to the default boot path

```sh
adb shell "mount -o remount,rw /"
adb shell "rm -rf /usr/share/qt-superbird-app/webapp/*"
adb push prototype/dashboard/frontend/index.html prototype/dashboard/frontend/style.css prototype/dashboard/frontend/app.js /usr/share/qt-superbird-app/webapp/
adb shell "mount -o remount,ro /"
```

This is the path Chromium's `supervisord` entry always loads on boot, so this makes the dashboard the default. If you want the dashboard reachable only on demand instead, with the bridge/button-test UI staying default, push it to its own separate path instead (for example `/usr/share/thing-revived-dashboard/`) and leave `/usr/share/qt-superbird-app/webapp/` alone.

### 3. fetch() vs XMLHttpRequest on file://, verified on the real device

The frontend polls the metrics file from a page loaded over `file://`, since there is no server process here by design, same reasoning as the main UI's persistence setup. This was tested directly against the on-device Chromium 69 over CDP, not assumed:

- `fetch('metrics.json')` on a `file://` page does retrieve the file body (confirmed: `.text()` returned the real content), but the `Response.status` is always `0` for `file://` URLs. That makes `Response.ok` permanently `false`, so the original `app.js` code (`if (!res.ok) throw ...`) would reject on every single poll and the dashboard would be stuck showing "NO SIGNAL" forever even though the data was actually arriving.
- Switching to `XMLHttpRequest` (which the existing chromium supervisord entry's `--allow-file-access-from-files` flag already supports) and judging success on getting parseable JSON back rather than on `xhr.status`, works correctly. Confirmed over CDP `Runtime.evaluate`: a synchronous `XMLHttpRequest` against the metrics file returned `status=0` but a full, correct `responseText`.
- `app.js` was changed to use `XMLHttpRequest` for this reason. No local HTTP server was needed, the fallback plan in this doc's original draft (a Python `http.server`, supervisord-managed) was not required.

This section's finding is about the frontend (unchanged) and Chromium 69's `file://` behavior, not the agent, so it is unaffected by the agent's later Python-to-Rust rewrite.

### 4. Metrics file location and the frontend's absolute file:// reference

`noc-agent`'s `OUTPUT_PATH` is `/var/thing-revived/noc-metrics.json` (not `/tmp`, not `/usr/share`, see the filesystem-writability note above). Because the frontend page lives in a different directory tree (`/usr/share/qt-superbird-app/webapp/`), `app.js`'s `METRICS_URL` is an absolute `file:///var/thing-revived/noc-metrics.json`, not a relative path. `--allow-file-access-from-files` permits this cross-directory `file://` to `file://` read; this was exercised for real over CDP, not assumed.

### 5. Viewing it

Since the dashboard now lives at the default boot path, it shows automatically after any reboot, no CDP navigation needed. To check or re-view it without rebooting:

```sh
adb forward tcp:2222 tcp:2222
curl localhost:2222/json   # find the active page's target id
# Page.navigate that target's websocket to file:///usr/share/qt-superbird-app/webapp/index.html
```

### What was actually verified on the real device, with a real reboot

**This section predates the Python-to-Rust rewrite of the agent** (it describes the deleted `noc-agent.py`) and has not been re-run against the physical unit since. The frontend, the deployment paths, and the filesystem-writability findings below are unchanged by the rewrite and remain accurate; the specific claim that the *agent process itself* was confirmed `RUNNING`/producing live data on real hardware applies to the Python version only. The new Rust `noc-agent` binary has only been run and verified on a desktop Linux host (see "What was actually verified on this host" further below); it has not yet been pushed to or run on the physical device.

- Pushed `noc-agent.py` to `/opt/thing-revived/`, the frontend initially to its own separate path, and the new `thing-revived-noc-agent.conf` to `/etc/supervisor.d/`, then ran `supervisorctl reread && supervisorctl update`; `supervisorctl status` showed `thing-revived-noc-agent` as `RUNNING` alongside the existing `thing-revived-bridge` and `chromium` entries.
- First attempt at the writable-path question actually failed in testing: pointing the metrics `OUTPUT_PATH` at a path under `/usr/share` worked fine until a real `adb reboot`, after which the agent's log (`/var/log/thing-revived-noc-agent.info`) filled with `OSError: [Errno 30] Read-only file system`, because the earlier `mount -o remount,rw /` had not survived the reboot. This is what led to moving the output to `/var/thing-revived/noc-metrics.json` instead, confirmed writable across a real reboot afterward.
- Ran a real `adb reboot`, waited for the device to reappear in `adb devices`, then ran `supervisorctl status` with no manual steps: both `thing-revived-noc-agent` and `thing-revived-bridge` showed `RUNNING`.
- Read `/var/thing-revived/noc-metrics.json`'s `ts` field twice, 4 seconds apart, immediately after that reboot: the value advanced by about 4000ms, matching the agent's 2 second poll interval, confirming the metrics file is being actively and continuously written purely by the supervisord-managed process, with nobody manually starting anything.
- Initially verified the dashboard by navigating the kiosk's existing page to its separate path over CDP and screenshotting it: fully rendered with real live data, CPU and memory gauges, disk bars, I/O sparkline, load averages, Bluetooth status, and a green "LIVE" indicator, all matching the device's actual state. Navigated back afterward to leave the device on the bridge/button-test UI, the default at that point.
- The dashboard was then moved to the default boot path itself (swapping with the bridge/button-test UI, per [`getting-started.md`](getting-started.md#swapping-which-app-is-the-default)) and the swap was confirmed with another real `adb reboot`: `curl localhost:2222/json` showed the active page title as the dashboard's own title with no manual navigation, and a fresh CDP screenshot again showed it fully rendered with live data, this time as the device's actual default boot screen.

## Quick iteration during development (temporary, does not survive reboot)

For fast edit-test loops while developing the frontend or agent without touching the persistent deployment above, the original manual-serve approach is still useful. Build the agent binary and push it and the static frontend files to the device, then start the collector and an HTTP server:

```sh
cd prototype/dashboard/agent
RUSTFLAGS="-C target-feature=+crt-static" cargo build --release --target armv7-unknown-linux-gnueabihf
adb -s <serial> shell "mkdir -p /tmp/noc-dashboard"
adb -s <serial> push target/armv7-unknown-linux-gnueabihf/release/noc-agent /tmp/noc-dashboard/noc-agent
adb -s <serial> shell "chmod +x /tmp/noc-dashboard/noc-agent"
adb -s <serial> push ../frontend/index.html ../frontend/style.css ../frontend/app.js /tmp/noc-dashboard/
```

Starting the two on-device processes needs a persistent `adb shell` connection. On this device, backgrounding a command inside a single `adb shell "... &"` call does not survive the call returning, the device kills everything attached to that shell session once it closes, even with `nohup` or `setsid`. What does work is keeping the `adb shell` command itself running as a long-lived background job on the host machine, so the connection to the device is never closed:

```sh
adb -s <serial> shell "cd /tmp/noc-dashboard && ./noc-agent" &
adb -s <serial> shell "cd /tmp/noc-dashboard && busybox httpd -f -p 8081 -h ." &
```

(Any static file server works for the second line; `busybox httpd` is used here instead of the Python version's `python3 -m http.server` since this device's only `httpd`-capable thing left on it after the Rust rewrite is BusyBox's own, not a Python one. This substitution has not itself been re-verified on the physical device - see "What was actually verified on the real device" below, which predates it.)

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

### What was actually verified for the Rust rewrite (host only, no device attached)

No physical device was attached in the session that rewrote `noc-agent` from Python to Rust. What was actually run and checked, on a desktop Linux host:

- `cargo build` and `cargo build --release` for the host target (`x86_64-unknown-linux-gnu`) both completed with zero errors/warnings.
- `RUSTFLAGS="-C target-feature=+crt-static" cargo build --release --target armv7-unknown-linux-gnueabihf` completed successfully and produced a statically-linked ARM binary; `file` confirmed `ELF 32-bit LSB executable, ARM, EABI5 version 1 (SYSV), statically linked`.
- The host-target release binary was actually run (not just built) for several seconds on this sandbox's own `/proc`, `df`, and `hciconfig`: it produced a real, well-formed `noc-metrics.json` with live host CPU (varying percentages across cores), memory, disk (`/`), load average, and Bluetooth adapter data, all field names and nesting matching the Python original's shape (see "What changed in the Rust rewrite" above for the exact field list checked). `mmcblk0` I/O read 0 KB/s on both read and write, which is correct for this host - it has no `mmcblk0` block device - not a bug.
- This confirms the agent's core logic and JSON output shape on a host x86_64 Linux build; it does **not** confirm on-device ARM behavior, the supervisord entry, the `/var` writable-across-reboot behavior, or anything else that needs the physical unit. All of that remains exactly as verified for the Python version above, pending a real on-device run of the new binary.

## Known limitations

- I/O is read-only throughput for the whole `mmcblk0` device, not broken out per-partition. `/proc/diskstats` does expose per-partition numbers (`mmcblk0p1` through `mmcblk0p18`), but the device-level `mmcblk0` line is the only one that saw non-zero read/write activity during testing, so this was kept simple rather than adding four empty per-partition sparklines.
- Network interface state only covers `bnep0`, `usb0`, `eth0`, and `wlan0`. None of those exist on this unit right now, so that tile is currently a placeholder in practice until a Bluetooth PAN or USB network link is brought up, which is tracked separately in [`architecture.md`](architecture.md).
- In the manual-serve dev path above, the agent is started by hand over `adb shell` and is not supervised; if it or the HTTP server dies, nothing restarts it automatically. The persistent deployment path earlier in this document resolves this: both the agent and (if ever needed) a local server are supervisord-managed with `autostart=true` and `autorestart=true`, confirmed to come back up on their own after a real reboot.
