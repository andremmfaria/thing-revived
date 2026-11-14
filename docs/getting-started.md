# Getting Started: Running the Prototype

Current scope: hardware input (buttons + knob) to visible reaction on the device's own screen. No smart-home controller integration, no Bluetooth transport yet. See [`architecture.md`](architecture.md) for where this fits in the full design.

## Prerequisites

- The device connected over USB, visible to `adb devices` (a udev rule for `1d6b:1014` and `adb` authorization are required once per host, see [`research/hardware-verification.md`](research/hardware-verification.md)).
- Rust with the `armv7-unknown-linux-gnueabihf` target installed (`rustup target add armv7-unknown-linux-gnueabihf`).
- Node.js + npm for the `ui/` build.

Why `armv7`, not `aarch64`: the device's kernel reports `aarch64` but its userspace ABI is 32-bit `armhf`, see [`hardware.md`](hardware.md).

## Persistent deployment (survives reboot, no manual steps after setup)

This is the primary deployment path. It replaces the content the device's own `supervisord`-managed Chromium kiosk already auto-loads on every boot, so once set up, pulling power and plugging the device back in brings the UI and bridge daemon up on their own, with no host machine involved at all.

### 1. Back up whatever is currently on the device first

```sh
adb pull /usr/share/qt-superbird-app/webapp backups/current-webapp-backup/
```

Cheap insurance before overwriting anything. The device's own app content cannot otherwise be recovered once replaced.

### 2. Build and deploy the UI

```sh
cd ui
npm install
npm run build
adb push dist/. /usr/share/qt-superbird-app/webapp/
```

Two build-config requirements, both already set in `ui/vite.config.js`:

- `build.target: 'chrome69'`, because that's the Chromium version actually running on-device (see [`hardware.md`](hardware.md)); modern syntax like `??`/`?.` silently fails to execute with no console error otherwise.
- The build must NOT emit an ES module (`type="module"`) script tag. Chromium enforces strict MIME-type checking for module scripts, and a plain filesystem `file://` load (no HTTP server, which is the point of this deployment path) never provides a `Content-Type` header, so a module script fails to load entirely with the page staying blank. Vite's default output needs reconfiguring (a non-module/IIFE build target) to produce a plain `<script src="...">` tag instead.

Chromium's existing `supervisord` program entry already points at this exact path (`--app=file:///usr/share/qt-superbird-app/webapp/index.html`) unconditionally on every boot, so no further configuration is needed for the UI half of this to persist.

### 3. Build and deploy the bridge daemon, with a supervisord entry so it survives reboot

```sh
cd bridge
RUSTFLAGS="-C target-feature=+crt-static" cargo build --release --target armv7-unknown-linux-gnueabihf
adb shell "mkdir -p /opt/thing-revived"
adb push target/armv7-unknown-linux-gnueabihf/release/superbird-bridge /opt/thing-revived/superbird-bridge
adb shell "chmod +x /opt/thing-revived/superbird-bridge"
```

Use a persistent path like `/opt/thing-revived/`, not `/tmp` (confirmed to be `tmpfs` on this device and wiped on every reboot).

Add a new supervisord program definition as its own file under `/etc/supervisor.d/`, picked up automatically via the existing `[include] files = /etc/supervisor.d/*.conf` directive in `/etc/supervisord.conf`, with no need to edit that main file:

```ini
[program:thing-revived-bridge]
command=/opt/thing-revived/superbird-bridge
numprocs=1
autorestart=true
autostart=true
startsecs=0
priority=95
redirect_stderr=true
stdout_logfile=/var/log/thing-revived-bridge.info
stdout_logfile_maxbytes=1MB
stdout_logfile_backups=3
```

```sh
adb push thing-revived-bridge.conf /etc/supervisor.d/thing-revived-bridge.conf
adb shell "supervisorctl reread && supervisorctl update"
```

Static linking (`+crt-static`) avoids a glibc-version mismatch between the cross-toolchain and the device's older glibc, see [`hardware.md`](hardware.md).

### 4. Verify

```sh
adb reboot
# wait for the device to reappear in `adb devices`, usually under a minute
adb shell "supervisorctl status"
```

Confirmed working: both `chromium` (pointed at the new UI) and `thing-revived-bridge` show as `RUNNING` with no manual intervention after a real `adb reboot`, not just a page reload.

## Quick iteration during development (temporary, does not survive reboot)

For fast edit-test loops while developing, the older temporary approach is still useful: serve the build over a local HTTP server instead of overwriting the device's real webapp content, and point the existing kiosk's Chrome DevTools debugging port at it without touching anything persistent.

```sh
adb push dist/. /tmp/ui/
adb shell "cd /tmp/ui && nohup python3 -m http.server 8080 --bind 127.0.0.1 > /tmp/httpd.log 2>&1 &"
adb forward tcp:2222 tcp:2222
curl localhost:2222/json   # find the active page's target id
# then use the Chrome DevTools Protocol (Page.navigate) against that target's
# ws://localhost:2222/devtools/page/<id> to navigate to http://localhost:8080/
```

Because this serves over real HTTP, the module-script MIME-type issue above does not apply here; this path can use Vite's normal default build output. This does not survive a reboot or even navigating the kiosk tab elsewhere, which is exactly why the persistent deployment above exists instead for anything meant to actually stay running.

## Watch events from a dev machine instead (optional)

```sh
adb forward tcp:8765 tcp:8765
# open a browser to the built ui/dist pointed at ws://localhost:8765
```

## Known open item

Exact button/knob scancode-to-physical mapping is not yet confirmed. `evtest`/`getevent` aren't available on-device, so it requires watching the bridge's raw output (`{"type":"button","code":<n>,...}` / `{"type":"knob","delta":<n>}`) while physically pressing each control. See [`research/hardware-interaction.md`](research/hardware-interaction.md).
