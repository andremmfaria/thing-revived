# Getting Started: Running the Prototype

Current scope: hardware input (buttons + knob) to visible reaction on the device's own screen. No smart-home controller integration, no Bluetooth transport yet. See [`architecture.md`](architecture.md) for where this fits in the full design.

## Prerequisites

- The device connected over USB, visible to `adb devices` (a udev rule for `1d6b:1014` and `adb` authorization are required once per host, see [`research/hardware-verification.md`](research/hardware-verification.md)).
- Rust with the `armv7-unknown-linux-gnueabihf` target installed (`rustup target add armv7-unknown-linux-gnueabihf`).
- Node.js + npm for the `ui/` build.

Why `armv7`, not `aarch64`: the device's kernel reports `aarch64` but its userspace ABI is 32-bit `armhf`, see [`hardware.md`](hardware.md).

## 1. Build and run the bridge daemon

```sh
cd bridge
RUSTFLAGS="-C target-feature=+crt-static" cargo build --release --target armv7-unknown-linux-gnueabihf
adb push target/armv7-unknown-linux-gnueabihf/release/superbird-bridge /tmp/superbird-bridge
adb shell "chmod +x /tmp/superbird-bridge; nohup /tmp/superbird-bridge > /tmp/bridge.log 2>&1 &"
```

Static linking (`+crt-static`) avoids a glibc-version mismatch between the cross-toolchain and the device's older glibc, see [`hardware.md`](hardware.md).

## 2. Build and serve the UI

```sh
cd ui
npm install
npm run build
adb push dist/. /tmp/ui/
adb shell "cd /tmp/ui && nohup python3 -m http.server 8080 --bind 127.0.0.1 > /tmp/httpd.log 2>&1 &"
```

The UI targets Chromium 69 explicitly (`vite.config.js`, `build.target: 'chrome69'`) because that's the version actually running on-device, see [`hardware.md`](hardware.md).

## 3. Show it on the device's own screen (no flashing required)

The device's current firmware (DeskThing) leaves a Chrome DevTools remote-debugging port open. This lets the existing on-device Chromium be navigated to the local UI directly:

```sh
adb forward tcp:2222 tcp:2222
curl localhost:2222/json   # find the active page's target id
# then use the Chrome DevTools Protocol (Page.navigate) against that target's
# ws://localhost:2222/devtools/page/<id> to navigate to http://localhost:8080/
```

This was confirmed working end-to-end (CDP screenshot showing the rendered UI on the physical screen) without any firmware flash, see [`research/hardware-verification.md`](research/hardware-verification.md) for the evidence.

## 4. Watch events from a dev machine instead (optional)

```sh
adb forward tcp:8765 tcp:8765
# open a browser to the built ui/dist pointed at ws://localhost:8765
```

## Known open item

Exact button/knob scancode-to-physical mapping is not yet confirmed. `evtest`/`getevent` aren't available on-device, so it requires watching the bridge's raw output (`{"type":"button","code":<n>,...}` / `{"type":"knob","delta":<n>}`) while physically pressing each control. See [`research/hardware-interaction.md`](research/hardware-interaction.md).
