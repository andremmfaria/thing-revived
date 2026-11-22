# Current State: What's Actually Implemented and Running

This is the single canonical account of what is built and running today. [`overview.md`](overview.md), [`architecture.md`](architecture.md) and the top-level [`README.md`](../README.md) point here instead of restating these facts.

## Firmware: `yocto-superbird`

The device runs `yocto-superbird` with this project's overlay: `aarch64-unknown-linux-gnu` userspace, systemd-managed services and a Chromium 147 kiosk (verified on the device on 2026-11-22). The image boots with the USB-C port in host mode, so SSH over USB-CDC-NCM is not available at boot until the port is switched to the device role (details in [`hardware.md`](hardware.md)). Build and flash workflow: [`yocto-build-and-flash.md`](yocto-build-and-flash.md). Dependencies: [`dependency-surface.md`](dependency-surface.md).

## Repo layout

- `controller/`: Rust daemon, binary `controller-agent`, layered as a modular monolith with ports and adapters (`domain/`, `services/`, `adapters/`, `utils/`). Bridges buttons, knob and the settings backend to the frontend over a WebSocket on `127.0.0.1:8765`.
- `daemon/`: Rust terminal agent, binary `terminal-agent`. Runs PTY sessions and relays them over a WebSocket on `127.0.0.1:8766`.
- `frontend/`: the kiosk web app. Classic scripts with no build step, a statechart interaction layer, screens defined as data, and vendored xterm.js 5.3.0 and FitAddon 0.8.0 under `frontend/vendor/`.
- `config/`: `controller.toml`, `terminal.toml` and `frontend.toml`, plus `config/protocol/` with the golden JSON wire fixtures shared by the Rust and frontend tests. See [`config/README.md`](../config/README.md).
- `build/`: `build-image.sh`, `flash-image.sh`, the Yocto overlay recipes (`build/overlay/`), config tooling (`build/tools/`) and `build/artifact/` for built images (contents gitignored). See [`build/README.md`](../build/README.md).
- `deploy/`: one legacy supervisord conf for `thing-revived-terminal`, kept for historical and stock-firmware reference only.
- `docs/`: this documentation.

Each Rust component is a plain crate at its own top-level directory. A `LICENSE` (MIT) file sits at the repo root.

## Services on the device

Two systemd units run on the device: `controller-agent.service` (from the `controller-agent` recipe) and `thing-revived-terminal-agent.service` (from the `thing-revived-terminal-agent` recipe). Both bind loopback only, overridable with `CONTROLLER_BIND` and `TERMINAL_AGENT_BIND` (see the Security model in [`architecture.md`](architecture.md#security-model)). Their configuration is installed under `/etc/thing-revived/` (`controller.toml`, `terminal.toml`), and the frontend config is generated into the kiosk web app at image build time. The web app, including the vendored xterm.js assets, is the kiosk's default boot page (`superbird-kiosk-default-webapp` recipe), and the only client of both WebSockets is the kiosk's own Chromium 147 page.

## The terminal: `daemon/` + `frontend/`

The main deliverable: a vendored xterm.js terminal UI backed by a Rust PTY agent, with multiple live sessions, typed into from a Bluetooth-paired keyboard (pairing and typing confirmed on hardware) plus the device's buttons and knob for session chrome (next/prev session, interrupt, clear, respawn). Full design and wire protocol: [`terminal.md`](terminal.md).

## `controller/`: buttons, knob and settings backend

A live runtime dependency of the frontend, not a prototype. It reads the buttons and knob through `evdev`, grabs them (`EVIOCGRAB`) and replays session-chrome keys through `uinput`, and serves the settings-menu backend: Bluetooth (BlueZ over D-Bus), brightness, timezone and Wi-Fi. Button and knob scancodes, the grab and replay mechanism, and the Bluetooth, brightness and timezone backend are confirmed on real hardware: see [`research/button-knob-clock-design.md`](research/button-knob-clock-design.md) and [`research/hardware-verification.md`](research/hardware-verification.md).

- **Wi-Fi works on real hardware.** Verified 2026-11-22 with a D-Link DWA-171 (`rtw88_8821cu`) through a USB-C dock in USB host mode; the dock's Ethernet also works. The code is in `controller/src/domain/wifi/`, `controller/src/adapters/wpa_ctrl.rs` and `controller/src/services/wifi.rs`, with a Wi-Fi submenu in `frontend/`. The image side is the `thing-revived-wifi-config` overlay recipe (plus `thing-revived-wifi-udev`). Design, wire protocol and limitations: [`wifi.md`](wifi.md).
- **Clock sync is system NTP.** The image runs `systemd-timesyncd` (the `thing-revived-ntp-config` recipe). The daemon reports the clock status to the frontend from `timedatectl show -p NTPSynchronized`; there is no SSH probe. Verified on hardware on 2026-11-22: the clock syncs by itself once a network is up.
- **Scope.** No smart-home or Bluetooth-peripheral client is part of this project; `controller/Cargo.toml` has no such dependency. The terminal and settings menu are the deliverable.

## Tests

Run from the repo root. Counts as of this refresh.

| Command | Result |
|---|---|
| `cargo test` in `controller/` | 144 tests pass (128 unit, 16 across the `input_replay`, `layering`, `router_flow` and `wire_golden` integration tests) |
| `cargo test` in `daemon/` | 65 tests pass (56 unit, 9 across the `no_payload_logging`, `protocol_golden` and `pty_roundtrip` integration tests) |
| `node --test frontend/tests/*.test.js` | 133 tests: 132 pass, 1 skipped, 0 fail |
| `python3 build/tools/test_toml_to_js.py` | 14 tests pass |
| `node frontend/tests/e2e/menu-regression.mjs` | Headless Chromium against a mocked WebSocket; needs a playwright library and Chromium, and prints `SKIP` and exits 0 when either is missing (see `frontend/tests/README.md`) |

CI (`.github/workflows/ci.yml`) also runs `cargo clippy`, the config check and `node --check` over the frontend scripts.
