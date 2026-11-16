# Architecture

Full decision record, including the same-day revision once real RAM data came in: see [`research/architecture-decision.md`](research/architecture-decision.md) (**read the REVISION section at the top of that file first**: it supersedes the original pass preserved below it for the audit trail). This file summarizes the current, standing design only.

## Chosen design: Chromium kiosk + separate native bridge daemon

- **UI**: a React SPA, rendered by a Chromium kiosk instance on-device, `file://`-served, mirroring the pattern already proven on this exact hardware (see [`hardware.md`](hardware.md)).
- **Bridge daemon**: a separate, independently-supervised process (not a Chromium child) that owns all hardware I/O (knob, buttons; not touch, which Chromium handles natively) and all Bluetooth/smart-home-controller networking. The UI never touches evdev, Bluetooth, or holds a controller credential: it only exchanges small JSON messages with the daemon over a local WebSocket. Full data flow: [`research/hardware-interaction.md`](research/hardware-interaction.md).
- **Bridge daemon language**: Rust (`evdev` for input, `tungstenite` sync/threaded for the local WebSocket, confirmed by `prototype/control-surface/agent/Cargo.toml`). `bluer` for BlueZ/Bluetooth and an async `tokio-tungstenite` runtime were the original plan, chosen specifically for `bluer`'s official, actively-maintained BlueZ bindings (see [`research/hardware-interaction.md`](research/hardware-interaction.md#bridge-daemon-implementation-language) for why Go was rejected, materially weaker Classic-Bluetooth/RFCOMM support) — **but neither is implemented yet**: `prototype/control-surface/agent/Cargo.toml` has no Bluetooth crate today, and the daemon's WebSocket server is the same sync `tungstenite` crate `terminal/agent/` also uses, not `tokio-tungstenite`.
- **Transport to the smart-home controller**: Bluetooth PAN/BNEP as primary, USB-cable (CDC-NCM) as secondary/fallback, same application protocol over either. See [`research.md`](research/research.md) §3 to §5.

## Why not the alternatives

Two other candidate architectures were seriously evaluated and explicitly rejected for the production target (kept as fallbacks). See [`research/architecture-decision.md`](research/architecture-decision.md) §1 for the full elaboration of all three, including RAM budgets and process models:

- **QtWebEngine native shell**: tighter single-process coupling, but zero cross-compile precedent on this device's BSP. Kept as the first fallback if the Chromium-kiosk path hits friction.
- **Pure input-satellite (no on-device rendering)**: the lowest-risk option and the only one with direct production precedent elsewhere, but it does not deliver an on-device UI at all, so it's a safety net, not a target.

## Other components reusing this pattern

Two further on-device deliverables reusing this pattern, each independently supervised: a NOC-style **dashboard** (Rust metrics daemon + web frontend, see [`dashboard.md`](dashboard.md)) and a **terminal** (Rust PTY-to-WebSocket daemon + a real, vendored xterm.js + FitAddon frontend, see [`terminal.md`](terminal.md)). Both were originally cross-compiled to `armv7-unknown-linux-gnueabihf` on stock firmware; that target is now `aarch64-unknown-linux-gnu` since the 2026-11-16 `yocto-superbird` reflash (see [`yocto-build-and-flash.md`](yocto-build-and-flash.md#4-the-cross-compile-target-gotcha-this-bsp-is-genuinely-64-bit) and the REVISION note in [`hardware.md`](hardware.md)). Neither changes the architecture decision above; they are additional applications of it, not alternatives to it.

## Current implementation status

See [`getting-started.md`](getting-started.md) for what's actually built and running today: a proof-of-life slice of this architecture (hardware input to on-screen reaction only; no smart-home controller or Bluetooth client wired in yet).
