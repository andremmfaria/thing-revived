# Architecture

Full decision record: see [`research/architecture-decision.md`](research/architecture-decision.md) for the full reasoning trail. This file summarizes the current, standing design only.

## Chosen design: Chromium kiosk + separate native bridge daemon

- **UI**: a static HTML/CSS/JS app, rendered by a Chromium kiosk instance on-device, `file://`-served, mirroring the pattern already proven on this exact hardware (see [`hardware.md`](hardware.md)).
- **Bridge daemon**: a separate, independently-supervised process (not a Chromium child) that owns all hardware I/O (knob, buttons; not touch, which Chromium handles natively) and the settings-menu backend (Bluetooth, brightness, timezone). The UI never touches evdev directly: it only exchanges small JSON messages with the daemon over a local WebSocket. Full data flow: [`research/hardware-interaction.md`](research/hardware-interaction.md).
- **Bridge daemon language**: Rust (`evdev` for input, `tungstenite` sync/threaded for the local WebSocket, `zbus` for the BlueZ D-Bus settings-menu backend, per `controller/Cargo.toml`).
- **Target**: `aarch64-unknown-linux-gnu`.

## Why not the alternatives

Two other candidate architectures were seriously evaluated and explicitly rejected, kept as fallbacks. See [`research/architecture-decision.md`](research/architecture-decision.md) for the full elaboration of all three, including RAM budgets and process models:

- **QtWebEngine native shell**: tighter single-process coupling, but zero cross-compile precedent on this device's BSP. Kept as the first fallback if the Chromium-kiosk path hits friction.
- **Pure input-satellite (no on-device rendering)**: the lowest-risk option and the only one with direct production precedent elsewhere, but it does not deliver an on-device UI at all, so it's a safety net, not a target.

## Other components reusing this pattern

One further on-device deliverable reusing this pattern, independently supervised: a **terminal** (Rust PTY-to-WebSocket daemon + a real, vendored xterm.js + FitAddon frontend, see [`terminal.md`](terminal.md)). This doesn't change the architecture decision above; the terminal is an additional application of it, not an alternative to it.

## Current implementation status

See [`development.md`](development.md) for what's actually built and running today.
