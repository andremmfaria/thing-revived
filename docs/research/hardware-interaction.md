# Hardware Interaction Design: Architecture (b)

## Status: SUPERSEDED / historical

This document describes a smart-home-controller / action-mapping / React-Angular-UI bridge-daemon architecture that was **never built**. What actually shipped is a much narrower terminal-agent WebSocket bridge (`controller/agent/` + `terminal/agent/`) with no smart-home controller or Bluetooth client wired in. For what the hardware-interaction pipeline actually looks like today — the confirmed `EVIOCGRAB`/`uinput` button-translation mechanism and the real, confirmed scancode map (resolving this file's own "Open item carried over" below) — see [`research/button-knob-clock-design.md`](button-knob-clock-design.md). For the current system design, see [`architecture.md`](../architecture.md). Kept below for the historical record of the original design reasoning, not as a description of what's running.

Companion to `architecture-decision.md` (chosen architecture: Chromium kiosk + separate native bridge daemon + WebSocket relay to the page). This document describes exactly how physical hardware (knob, 5 buttons, touchscreen) reaches the smart-home controller and the on-device React/Angular UI under that architecture.

## Pipeline overview

1. **Kernel evdev nodes** (confirmed live on the physical unit, `hardware-verification.md`):
   - `/dev/input/event0`: `gpio-keys`, the 5 physical buttons, discrete `KEY_*` press/release events.
   - `/dev/input/event1`: `rotary@0`, the knob, reported as `REL_HWHEEL` relative-axis deltas, not discrete `KEY_LEFT`/`KEY_RIGHT`.
   - `/dev/input/event3`: `tlsc6x_dbg`, the touchscreen, I2C bus 0 addr 0x2e, standard `ABS` multitouch events.

2. **Touchscreen is NOT part of the hardware-bridge pipeline.** On-screen taps on the rendered React UI go straight into Chromium's own native input stack as ordinary pointer/touch events, exactly like any touchscreen kiosk. The bridge daemon only reads the knob and the 5 buttons; Chromium has no concept of a rotary knob, so that path is handled entirely outside the browser.

3. **Bridge daemon**: a separate process (own systemd unit, not a Chromium child) opens `event0` + `event1` directly. Needs group `input` read access, the same udev-rule pattern already used to unblock `adb` on this unit during verification.

4. **Translation layer** inside the daemon: raw `KEY_*` codes map to logical names (`BUTTON_1`..`BUTTON_5`, possibly knob-click if it shares `event0`'s bitmap; **unconfirmed, needs a live `evtest`/`getevent -lt` capture while physically pressing each control**, now straightforward since root adb shell access exists). Raw `REL_HWHEEL` deltas are debounced/coalesced into discrete "knob +1/-1 step" events rather than flooding on every tick.

5. **Action mapping**: a config the daemon owns (JSON/YAML, editable independent of UI code) maps each logical hardware event to a backend action, e.g. button 1 triggers turning on a given light entity, knob rotate adjusts brightness via a step service call.

6. **The smart-home controller connection lives in the daemon, not the browser.** The daemon holds the persistent connection to the smart-home controller (long-lived access token stays server-side, never shipped to the page's JS, a real security property, not just tidiness), over whichever transport is currently up per the BT-PAN-primary/cable-secondary failover logic (`research.md` §5). On a mapped button/knob event it calls the backend action directly, and subscribes to the backend's state-change events to push relevant updates toward the UI.

7. **Bridge-to-page channel**: the daemon also runs a small local WebSocket server (`ws://localhost:PORT`, pure loopback, no network exposure) that the React/Angular app connects to on load. App-defined JSON protocol, outbound e.g. `{"type":"knob","delta":1}` for UI feedback, inbound e.g. `{"type":"state","entity":"light.living_room","state":"on"}` to trigger a re-render. Recommendation: route the page's own backend-triggered actions (on-screen dashboard buttons, not physical-hardware-triggered) through this same bridge relay too, rather than giving the browser its own separate backend token, keeping credentials and connection-state logic in exactly one place.

## Net effect

The React/Angular app never talks to evdev, never talks to Bluetooth, never holds a backend token. It is a display layer that sends/receives small JSON messages over localhost. All hardware I/O and backend protocol logic sits in one daemon process, which is the actual implementation target.

## Bridge daemon implementation language

Python (asyncio + `python-evdev`) was the default in earlier research purely because it avoids cross-compilation (the interpreter runs on-device already). It is not required.

Checked (web search, 2026-11-14) whether Rust or Go are viable, more efficient alternatives:

- **Rust**: `bluer` (docs.rs/bluer, github.com/bluez/bluer) is the **official** BlueZ binding for Rust, actively maintained by the BlueZ project itself, async via `tokio`, supports L2CAP and RFCOMM sockets with a Tokio-networking-like API. An `evdev` crate exists for input handling. Cross-compiling Rust to aarch64 against a Yocto `populate_sdk` toolchain is a well-documented, well-trodden pattern (Rust's own ecosystem docs explicitly call out Buildroot/Yocto integration). Binary footprint: single-digit MB typical for this kind of daemon, vs. Python's roughly 15 to 30MB estimate.
- **Go**: evdev bindings exist, but the Bluetooth/BlueZ ecosystem is **materially weaker for Classic Bluetooth** specifically. A direct source doing equivalent BlueZ/RFCOMM work in Go states plainly that Go's BT libraries (`go-bluetooth`, `gatt`) "mainly targeted Bluetooth LE" and that "RFCOMM (and thus SPP) is only supported in Bluetooth Classic" with weaker library support there. Since this project's primary transport is BT-PAN/RFCOMM (Classic BT, not BLE), this is a real gap, not a stylistic preference.

**Recommendation: Rust**, specifically because of `bluer`'s BlueZ coverage (not just "compiled languages are faster than Python" in general): `evdev` crate for input, `bluer` for the BT-PAN/RFCOMM stack, `tokio-tungstenite` for the local WebSocket bridge to the page, a small WebSocket client against the backend's JSON protocol. Memory-safety is a secondary but real benefit for an always-on, unattended daemon. RAM footprint is not the dominant reason given the already-validated 323MB headroom (`hardware-verification.md`), but a single-digit-MB compiled daemon is still strictly better than a 15 to 30MB interpreter-based one for a component meant to be the thin, always-on half of the architecture.

## Open item carried over

Exact scancode-to-physical-button mapping, and whether knob-click lives in `event0`'s bitmap, are unconfirmed. Capture with `evtest`/`getevent -lt` on the live unit while pressing each control, before finalizing the translation layer in step 4.
