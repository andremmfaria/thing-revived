# Getting Started: Running the Prototype

Current scope: hardware input (buttons + knob) to visible reaction on the device's own screen. No smart-home controller integration, no Bluetooth transport yet. See [`architecture.md`](architecture.md) for where this fits in the full design.

Note on the device's default boot screen: as of this writing, the [NOC dashboard](dashboard.md) occupies the default boot path and shows automatically on power-up. This bridge/button-test UI is deployed to its own separate path instead and is reached on demand through the same CDP `Page.navigate` trick described below, not shown automatically. Which of the two is the default is a deliberate, swappable choice, not a fixed property of either project; see "Swapping which app is the default" below for how to change it.

## Prerequisites

- The device connected over USB, visible to `adb devices` (a udev rule for `1d6b:1014` and `adb` authorization are required once per host, see [`research/hardware-verification.md`](research/hardware-verification.md)).
- Rust with the `armv7-unknown-linux-gnueabihf` target installed (`rustup target add armv7-unknown-linux-gnueabihf`).
- Node.js + npm for the `prototype/control-surface/frontend/` build.

Why `armv7`, not `aarch64`: the device's kernel reports `aarch64` but its userspace ABI is 32-bit `armhf`, see [`hardware.md`](hardware.md).

## Persistent deployment (survives reboot, no manual steps after setup)

This is the primary deployment path. Chromium's `supervisord` program entry always auto-loads whatever is at a single fixed path, `/usr/share/qt-superbird-app/webapp/index.html`, on every boot with no configuration change needed. Whichever app's content occupies that path is the one shown by default; the other app lives at its own separate path and is reached on demand. This section deploys the bridge/UI prototype to its own separate path, `/usr/share/thing-revived-uidemo/`, alongside the dashboard's default-path arrangement described in [`dashboard.md`](dashboard.md).

### 1. Back up whatever is currently on the device first

```sh
adb pull /usr/share/qt-superbird-app/webapp backups/current-webapp-backup/
```

Cheap insurance before overwriting anything.

### 2. Build and deploy the UI to its own path

```sh
cd prototype/control-surface/frontend
npm install
npm run build
adb shell "mount -o remount,rw /"
adb shell "mkdir -p /usr/share/thing-revived-uidemo"
adb push dist/. /usr/share/thing-revived-uidemo/
adb shell "mount -o remount,ro /"
```

Two build-config requirements, both already set in `prototype/control-surface/frontend/vite.config.js`:

- `build.target: 'chrome69'`, because that's the Chromium version actually running on-device (see [`hardware.md`](hardware.md)); modern syntax like `??`/`?.` silently fails to execute with no console error otherwise.
- The build must NOT emit an ES module (`type="module"`) script tag. Chromium enforces strict MIME-type checking for module scripts, and a plain filesystem `file://` load (no HTTP server, which is the point of this deployment path) never provides a `Content-Type` header, so a module script fails to load entirely with the page staying blank. Vite's default output needs reconfiguring (a non-module/IIFE build target) to produce a plain `<script src="...">` tag instead.

### 3. Build and deploy the bridge daemon, with a supervisord entry so it survives reboot

```sh
cd prototype/control-surface/agent
RUSTFLAGS="-C target-feature=+crt-static" cargo build --release --target armv7-unknown-linux-gnueabihf
adb shell "mkdir -p /opt/thing-revived"
adb push target/armv7-unknown-linux-gnueabihf/release/superbird-bridge /opt/thing-revived/superbird-bridge
adb shell "chmod +x /opt/thing-revived/superbird-bridge"
```

Use a persistent path like `/opt/thing-revived/`, not `/tmp` (confirmed to be `tmpfs` on this device and wiped on every reboot).

Add a new supervisord program definition as its own file under `/etc/supervisor.d/`, picked up automatically via the existing `[include] files = /etc/supervisor.d/*.conf` directive in `/etc/supervisord.conf`, with no need to edit that main file. The source for this is kept in the repo at [`prototype/control-surface/deploy/supervisor.d/thing-revived-bridge.conf`](../prototype/control-surface/deploy/supervisor.d/thing-revived-bridge.conf):

```sh
adb push prototype/control-surface/deploy/supervisor.d/thing-revived-bridge.conf /etc/supervisor.d/thing-revived-bridge.conf
adb shell "supervisorctl reread && supervisorctl update"
```

Static linking (`+crt-static`) avoids a glibc-version mismatch between the cross-toolchain and the device's older glibc, see [`hardware.md`](hardware.md).

### 4. Verify

```sh
adb reboot
# wait for the device to reappear in `adb devices`, usually under a minute
adb shell "supervisorctl status"
```

Confirmed working: `thing-revived-bridge` shows as `RUNNING` with no manual intervention after a real `adb reboot`, not just a page reload.

### 5. View this UI on demand, since it is not the current default

```sh
adb forward tcp:2222 tcp:2222
curl localhost:2222/json   # find the active page's target id
# then use the Chrome DevTools Protocol (Page.navigate) against that target's
# ws://localhost:2222/devtools/page/<id> to navigate to
# file:///usr/share/thing-revived-uidemo/index.html
```

## Swapping which app is the default

Whichever content sits at `/usr/share/qt-superbird-app/webapp/` is what shows automatically on boot; the other app sits at its own path and is reached on demand. To make this UI the default instead of the dashboard, swap the two directories' contents:

```sh
adb shell "mount -o remount,rw /"
adb shell "mkdir -p /usr/share/thing-revived-dashboard-backup"
adb shell "cp -r /usr/share/qt-superbird-app/webapp/. /usr/share/thing-revived-dashboard-backup/"
adb shell "rm -rf /usr/share/qt-superbird-app/webapp/*"
adb shell "cp -r /usr/share/thing-revived-uidemo/. /usr/share/qt-superbird-app/webapp/"
adb shell "mount -o remount,ro /"
adb reboot
```

Then update this doc and [`dashboard.md`](dashboard.md) to reflect which app is now at which path; the two documents should always describe reality, not a fixed assumption about which one is default.

## Quick iteration during development (temporary, does not survive reboot)

For fast edit-test loops while developing, a lighter-weight approach is still useful: serve the build over a local HTTP server instead of pushing to any persistent path, and point the existing kiosk's Chrome DevTools debugging port at it.

```sh
adb push dist/. /tmp/frontend/
adb shell "cd /tmp/frontend && nohup busybox httpd -f -p 8080 -h . > /tmp/httpd.log 2>&1 &"
adb forward tcp:2222 tcp:2222
curl localhost:2222/json   # find the active page's target id
# then use the Chrome DevTools Protocol (Page.navigate) against that target's
# ws://localhost:2222/devtools/page/<id> to navigate to http://localhost:8080/
```

Because this serves over real HTTP, the module-script MIME-type issue above does not apply here; this path can use Vite's normal default build output. This does not survive a reboot or even navigating the kiosk tab elsewhere, which is why the persistent deployment above exists for anything meant to actually stay running. `busybox httpd` is used here rather than `python3 -m http.server`, since no Python toolchain or runtime is used anywhere in this project (see [`dashboard.md`](dashboard.md#quick-iteration-during-development-temporary-does-not-survive-reboot) for the same substitution made for the NOC dashboard's dev-iteration path); the device's BusyBox already ships `httpd`, confirmed present, so this adds no new on-device dependency.

## Watch events from a dev machine instead (optional)

```sh
adb forward tcp:8765 tcp:8765
# open a browser to the built prototype/control-surface/frontend/dist pointed at ws://localhost:8765
```

## Known open item

Exact button/knob scancode-to-physical mapping was captured by the user through a live hardware test and confirmed working correctly. See [`research/hardware-interaction.md`](research/hardware-interaction.md) for the pipeline this feeds into.
