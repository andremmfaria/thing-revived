# Current State: What's Actually Implemented and Running

This is the single canonical account of what's actually built and running today, consolidated from README.md's old "Layout" status callouts, `overview.md`'s old "Current state" section, and `architecture.md`'s old "Current implementation status" section — those three now point here instead of each independently restating (and drifting from) the same facts.

## Firmware: `yocto-superbird`

The device runs `yocto-superbird`, documented in full in [`yocto-build-and-flash.md`](yocto-build-and-flash.md): `ssh` over USB-CDC-NCM, `aarch64-unknown-linux-gnu` userspace, systemd-managed services, and a Chromium kiosk build. See [`hardware.md`](hardware.md) for the hardware facts and [`dependency-surface.md`](dependency-surface.md) for the full dependency picture.

## Repo layout

The repo root holds `daemon/` (the terminal's PTY-relay Rust agent, binary `terminal-agent`), `frontend/` (the terminal's static HTML/CSS/JS UI), `controller/` (the buttons/knob bridge Rust daemon, binary `controller-agent`), `deploy/` (the one surviving legacy supervisord conf, for `thing-revived-terminal`, kept for historical/stock-firmware reference only), and `docs/`. Each Rust component is a plain crate at its own top-level directory, no intermediate `agent/` subdirectory.

## `daemon/` + `frontend/` (the terminal) is the main active project

A real, vendored xterm.js 5.3.0 + FitAddon 0.8.0 terminal UI (`frontend/`), backed by a Rust PTY agent (`daemon/`, binary `terminal-agent`), reachable from a Bluetooth-paired keyboard plus the device's own buttons/knob for session chrome (next/prev session, interrupt, clear, respawn). Full design and wire protocol: [`terminal.md`](terminal.md).

Both `terminal-agent` and the buttons/knob bridge daemon `controller-agent` (see below) run as systemd-managed services baked directly into the custom webapp that is the kiosk's default boot page, including the vendored xterm.js assets (see [`yocto-build-and-flash.md`](yocto-build-and-flash.md) for the recipe details). `terminal-agent.service` and `controller-agent.service` both run on the device, with multiple live terminal sessions and both WebSocket ports (8765 bridge, 8766 terminal) listening with active connections from the kiosk's own Chromium page. BT-keyboard pairing is confirmed working: paired, and typing from a real BT keyboard into all live terminal sessions.

## `controller/` — the buttons/knob bridge daemon, live and current

A Rust daemon (`controller/`, binary `controller-agent`, uses `evdev` for input) relaying button/knob input events to `frontend/`'s session-chrome handler over a local WebSocket, plus the settings-menu backend (Bluetooth, brightness, timezone). It is one of the two packages shipped into the current `yocto-superbird` kiosk image, because `frontend/`'s own session-chrome input depends on it running — not a prototype kept for reference, a live runtime dependency.

- **Bluetooth/smart-home-controller client: out of scope, dropped.** `Cargo.toml` has no Bluetooth-peripheral/smart-home crate and none is planned — this was the project's original premise (see [`architecture.md`](architecture.md#chosen-design-chromium-kiosk--separate-native-bridge-daemon)) but was explicitly cut from scope in favor of the terminal becoming the main deliverable.
- Button/knob scancodes, the `EVIOCGRAB`+`uinput` session-chrome mechanism, and the Bluetooth/brightness/timezone settings-menu backend are all confirmed live on real hardware: see [`research/button-knob-clock-design.md`](research/button-knob-clock-design.md) and [`research/hardware-verification.md`](research/hardware-verification.md).

## Which project is "the main one"

The terminal (`daemon/` + `frontend/`) is the main active project going forward. `controller/` is not a prototype — it's a live runtime dependency of the terminal's session chrome and settings menu, shipped in the same kiosk image under its own CI and its own recipe/unit.
