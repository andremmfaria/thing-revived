# Architecture

This file describes the current, standing design. The reasoning behind the choice of design, including the rejected alternatives, is in [`research/architecture-decision.md`](research/architecture-decision.md). What is built and running today is tracked in [`development.md`](development.md).

## Overview

The device runs a Chromium 147 kiosk (verified on the device on 2026-11-22) that loads a static web app from `file://`, plus two independently supervised Rust agents that the app talks to over loopback WebSockets. The agents own everything the browser cannot do: raw input, Bluetooth, Wi-Fi, brightness, timezone and shell sessions. The app never touches hardware directly.

| Component | Directory | Role | Listens on (default) |
|---|---|---|---|
| controller-agent | `controller/` | Buttons and knob, settings backend (Bluetooth, Wi-Fi, brightness, timezone) | `127.0.0.1:8765` |
| terminal-agent | `daemon/` | PTY sessions relayed over WebSocket, clock sync status | `127.0.0.1:8766` |
| kiosk web app | `frontend/` | Terminal tabs, settings menu, status bar | none (WebSocket client) |

Both agents are built for `aarch64-unknown-linux-gnu` with `+crt-static`, run as systemd services and are not children of the browser. Touch is handled natively by Chromium, so the controller only handles the six buttons and the knob.

## Controller (`controller/`)

The controller is a layered Rust crate. Dependencies point inward only, and `controller/tests/layering.rs` enforces it.

- `domain/`: pure logic with no I/O (Bluetooth classification and pairing state, brightness, timezone, key mapping, long-press detection, and the Wi-Fi scan, status, validation, event and tracker modules).
- `ports.rs`: the traits the inner layers use to reach the outside (Bluetooth, settings store, event sink, restarter and so on).
- `services/`: use cases built on the ports (Bluetooth, display, input button policy, settings, timezone, Wi-Fi, and the accounts and first-boot services).
- `app/`: composition and dispatch (startup, the event hub that fans events out to clients, the command router, wire mapping, error and policy types). `lib.rs` is a thin composition root and the only place that picks concrete adapters.
- `adapters/`: the implementations of the ports (evdev input, uinput replay, BlueZ over D-Bus with `zbus`, an unavailable-Bluetooth fallback, sysfs backlight, `systemctl`, `timedatectl`, JSON settings file, the `wpa_supplicant` control socket, the account adapters for `/etc/shadow`, `unix_chkpwd`, `ssh-keygen` and the key store, and the WebSocket server).
- `protocol/`: the JSON command and event types.
- `utils/`: configuration loading, logging and `types.rs`, the central type catalogue.

The WebSocket server uses synchronous `tungstenite` with threads and no async runtime. The agent grabs the button evdev node exclusively and leaves the knob ungrabbed. A long press of KEY_M restarts the terminal agent unit. While the first-boot password screen is active the button policy consumes M and the knob push at the evdev reader instead (no uinput replay, no long-press restart), so only the physical M press can generate or acknowledge the password. See [`users.md`](users.md). Wi-Fi design and menu behavior are in [`wifi.md`](wifi.md).

## Terminal agent (`daemon/`)

The terminal agent is a Rust PTY-to-WebSocket relay on `tungstenite`. It runs one PTY session per `[[session]]` table in `terminal.toml` (four login shells by default), respawns exited sessions according to its respawn policy, and relays input, output and resize messages. A separate `clock/` module reports whether the system clock is NTP synchronized as `clock_sync` messages. The device clock syncs over NTP once it has network. The handshake enforces an Origin policy: no Origin, `null` (what Chromium sends for `file://` pages) and `localhost` or `127.0.0.1` over plain `http` are accepted, everything else is rejected. See [`terminal.md`](terminal.md) for details.

## Frontend (`frontend/`)

The frontend is a classic-script kiosk app: no ES modules and no bundler. The page loads from `file://`, where ES modules are expected to be blocked (not yet tested on the device), and the same files load under node for the unit tests. The browser is Chromium 147 (verified on the device on 2026-11-22), so the older Chromium 68 or 69 syntax ceiling the code was written to is a conservative baseline kept by habit, not a browser requirement; see the [Chromium 147 frontend audit](research/chromium-147-frontend-audit.md). Scripts are listed in load order in `index.html`, and each module also loads under Node so the unit tests can run without a browser. xterm.js and its FitAddon are vendored under `frontend/vendor/`.

- `core/`: module registry, event bus and timers.
- `transport/`: `channel.js` (reconnecting WebSocket with a bounded queue of idempotent getters), `agent-client.js` for the terminal agent and `bridge-client.js` for the controller.
- `state/`: store, reducers and selectors for the domain state.
- `interaction/`: the statechart layer. `machine.js` is a small table-driven statechart runner, `menu-chart.js` defines the knob, button and menu states, and `effects.js` interprets the effects the chart returns. Transitions are pure functions of state and event, and effects (including timers) are plain data, so the whole interaction is testable without a DOM.
- `menu/screens/`: screens as data. Each screen module (root, Bluetooth, Wi-Fi, timezone, terminal, Users and Groups) exports row definitions and controller-event reactions, and `index.js` merges them into one registry the chart consumes. `menu-view.js` renders whatever the registry describes.
- `firstboot/`: the full-screen first-boot password overlay. It sits outside the menu stack so no menu route can reach or dismiss it, and it gates keyboard, touch, chart input and `sendInput` while it is up.
- `terminal/`, `wifi/`, `input/`, `clock/`: the terminal sessions and views, the Wi-Fi picker, input handling and the status bar clock.
- `utils/`: configuration, logging and `types.js`, the central type catalogue.

## Configuration flow

Runtime configuration lives in `config/`, one TOML file per component: `controller.toml`, `terminal.toml` and `frontend.toml`. Each file documents every key and holds the compiled-in defaults, and tests keep the two identical. `config/README.md` has the full description, including how to change a value on a running device.

- The image installs `controller.toml` and `terminal.toml` under `/etc/thing-revived/`. The systemd units point the agents at them through `CONTROLLER_CONFIG` and `TERMINAL_AGENT_CONFIG`. A missing file means built-in defaults. An invalid file makes the agent exit with status 78.
- The kiosk cannot read TOML. At image build time `build/tools/toml_to_js.py` converts `frontend.toml` into `config.generated.js` (`window.THING_CONFIG = {...}`), which `index.html` loads before the other scripts and installs under `/usr/share/superbird-kiosk-default/`. The committed `frontend/config.generated.js` is an empty stub, so the app also runs from the repo with defaults.
- `build/build-image.sh` validates all three files before compiling anything. See [`build/README.md`](../build/README.md) and [`yocto-build-and-flash.md`](yocto-build-and-flash.md).

## Wire protocol

The frontend and the agents exchange small JSON messages over two loopback WebSockets: the controller bridge (default `ws://127.0.0.1:8765`) and the terminal agent (default `ws://127.0.0.1:8766`). The URLs are set in `frontend.toml`. `config/protocol/` holds golden fixtures, one file per message variant, in `controller/` and `terminal/` subdirectories. The Rust tests in `controller/` and `daemon/` and the frontend unit and end-to-end tests all read the same files, so a wire-format change has to touch a fixture and cannot drift on one side. The fixtures are test data only and are not installed on the device. Naming and the procedure for adding a message are in `config/protocol/README.md`.

## Security model

Both agents listen on loopback only by default: `controller-agent` on `127.0.0.1:8765` (override with `CONTROLLER_BIND` or `[server] bind`) and `terminal-agent` on `127.0.0.1:8766` (override with `TERMINAL_AGENT_BIND` or `[server] bind`). The kiosk frontend, running in the on-device Chromium, is the only client. Neither WebSocket has TLS or authentication, and the terminal agent is an unauthenticated shell that runs as `kiosk` (with its `sudo` and `adm` groups), so loopback is the actual access control. Chromium itself runs as root with `--no-sandbox` and a loopback debugging port, see [`hardware.md`](hardware.md#browser-security-limitation). Widening a bind (for example to `0.0.0.0`) is an explicit opt-in and should only be done on a trusted network.

For remote development, forward the ports over SSH instead of exposing them: `ssh -L 8765:127.0.0.1:8765 -L 8766:127.0.0.1:8766 kiosk@<device>`. Root login is refused; `kiosk` signs in with its password or a key, see [`users.md`](users.md). This needs a network path to the device. The image boots with the USB-C port in host mode, so the USB gadget (CDC-NCM) link is down at boot. Use it only after switching the port to the device role, or reach the device over a dock network (Ethernet, or a Wi-Fi adapter). See [`hardware.md`](hardware.md) for the roles and [`yocto-build-and-flash.md`](yocto-build-and-flash.md) for the SSH-over-USB steps.
