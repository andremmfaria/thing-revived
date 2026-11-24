# Current State: What's Actually Implemented and Running

This is the single canonical account of what is built and running today. [`overview.md`](overview.md), [`architecture.md`](architecture.md) and the top-level [`README.md`](../README.md) point here instead of restating these facts.

## Firmware: `yocto-superbird`

The device runs `yocto-superbird` with this project's overlay: `aarch64-unknown-linux-gnu` userspace, systemd-managed services and a Chromium 147 kiosk (verified on the device on 2026-11-22). The image boots with the USB-C port in device mode, so SSH over USB-CDC-NCM is available at boot; host mode for dock adapters is opt-in with `superbird-usb-role persist host` and takes effect from the next boot (details in [`hardware.md`](hardware.md)). In the device role the NCM dev link exists even with adb off (the `thing-revived-usb-bind` recipe binds the gadget), and you reach the kiosk with `ssh kiosk@<device>` and its password; `adbd` stays a dev-only build flag (`THING_REVIVED_ENABLE_ADB=1`). Build and flash workflow: [`yocto-build-and-flash.md`](yocto-build-and-flash.md). Dependencies: [`dependency-surface.md`](dependency-surface.md). Accounts, passwords and SSH: [`users.md`](users.md).

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

Two application units run on the device: `controller-agent.service` (from the `controller-agent` recipe) and `thing-revived-terminal-agent.service` (from the `thing-revived-terminal-agent` recipe). The terminal agent runs as the `kiosk` user, not root. The accounts recipe (`thing-revived-accounts`) adds `thing-revived-etc.service` (mounts the `/etc` overlay) and `thing-revived-provision.service` (boot-time account state check). Both bind loopback only, overridable with `CONTROLLER_BIND` and `TERMINAL_AGENT_BIND` (see the Security model in [`architecture.md`](architecture.md#security-model)). Their configuration is installed under `/etc/thing-revived/` (`controller.toml`, `terminal.toml`), and the frontend config is generated into the kiosk web app at image build time. The web app, including the vendored xterm.js assets, is the kiosk's default boot page (`superbird-kiosk-default-webapp` recipe), and the only client of both WebSockets is the kiosk's own Chromium 147 page.

## The terminal: `daemon/` + `frontend/`

The main deliverable: a vendored xterm.js terminal UI backed by a Rust PTY agent, with multiple live sessions, typed into from a Bluetooth-paired keyboard (pairing and typing confirmed on hardware) plus the device's buttons and knob for session chrome (next/prev session, interrupt, clear, respawn). Full design and wire protocol: [`terminal.md`](terminal.md).

## `controller/`: buttons, knob and settings backend

A live runtime dependency of the frontend, not a prototype. It reads the buttons and knob through `evdev`, grabs them (`EVIOCGRAB`) and replays session-chrome keys through `uinput`, and serves the settings-menu backend: Bluetooth (BlueZ over D-Bus), brightness, timezone and Wi-Fi. Button and knob scancodes, the grab and replay mechanism, and the Bluetooth, brightness and timezone backend are confirmed on real hardware: see [`research/button-knob-clock-design.md`](research/button-knob-clock-design.md) and [`research/hardware-verification.md`](research/hardware-verification.md).

- **Wi-Fi works on real hardware.** Verified 2026-11-22 with a D-Link DWA-171 (`rtw88_8821cu`) through a USB-C dock in USB host mode; the dock's Ethernet also works. The code is in `controller/src/domain/wifi/`, `controller/src/adapters/wpa_ctrl.rs` and `controller/src/services/wifi.rs`, with a Wi-Fi submenu in `frontend/`. The image side is the `thing-revived-wifi-config` overlay recipe (plus `thing-revived-wifi-udev`). Design, wire protocol and limitations: [`wifi.md`](wifi.md).
- **Clock sync is system NTP.** The image runs `systemd-timesyncd` (the `thing-revived-ntp-config` recipe). The daemon reports the clock status to the frontend from `timedatectl show -p NTPSynchronized`; there is no SSH probe. Verified on hardware on 2026-11-22: the clock syncs by itself once a network is up.
- **Accounts and first-boot password.** Implemented in the repo, not yet built into an image or run on a device (status: implemented, pending the first build). The image ships `kiosk` locked with no SSH key baked. If `kiosk` has no usable password, the frontend shows a full-screen "Kiosk password not set" message with an arrow at the M button. The first M press makes the controller generate and set a password and the screen shows it, with no toast and no timeout. The second M press acknowledges. A reboot before the acknowledgement locks the password again and the unset screen returns. The password lives in controller memory only, and the only stored artefact is its hash. The controller owns the phase machine (`none`, `prompt`, `generating`, `showing`, `done`) and decides at the evdev reader, so only the physical M press counts. Design: [`users.md`](users.md).
- **SSH and sudo.** `kiosk` signs in by password or by a key from the key store, with `Match User kiosk` and `AuthenticationMethods password publickey` in the sshd drop-in. Root is locked and refused, and every other user is key-only. `pam_faillock` is set to `deny=5 unlock_time=600 fail_interval=900`. The on-device unlock path verifies passwords with `unix_chkpwd`, which bypasses faillock, so the app's own rate limiter is the brake there. Terminal shells run as `kiosk` and so carry its `sudo` and `adm` groups (systemd calls `initgroups()` whenever `User=` is set). Account changes persist through an overlay on `/etc` with its upper layer on `/var`.
- **USB mode menu.** Settings, USB mode: shows the running and the saved USB-C role, saves Device or Host for the next boot (`superbird-usb-role persist`, never a live switch) and has a hold-to-confirm Reboot now. It is how host mode (USB Wi-Fi, Ethernet, storage through a dock) is enabled without a shell, and it drops the USB link to a PC after the next boot, so the screen warns about it. Code: `controller/src/domain/usb_role.rs`, `controller/src/services/usb_role.rs`, `controller/src/adapters/usb_role_cli.rs` and `frontend/src/menu/screens/usb-role.js`. Behavior, ADB dev build handling and recovery: [`hardware.md`](hardware.md#usb-mode-in-the-settings-menu). The `superbird-usb-role persist host` command on the device stays the fallback. To try a change on a device, cross-compile the controller (`aarch64-unknown-linux-gnu`, static CRT) and copy the binary plus the `frontend/` files over the dev link before switching to host mode, since host mode removes that link.
- **Users menu.** Settings, Users: admin unlock, users (add, lock, set password, remove), groups, SSH keys, with hold-to-confirm for destructive actions. Wire protocol and error codes: [`../config/protocol/controller/README.md`](../config/protocol/controller/README.md). Defaults and limits: the `[users]` section in [`../config/README.md`](../config/README.md).
- **Scope.** No smart-home or Bluetooth-peripheral client is part of this project; `controller/Cargo.toml` has no such dependency. The terminal and settings menu are the deliverable.

## Tests

Run from the repo root. Counts as of this refresh.

| Command | Result |
|---|---|
| `cargo test` in `controller/` | 378 tests pass (328 unit, 50 across the `input_replay`, `layering`, `log_policy`, `no_secret_logging`, `router_flow` and `wire_golden` integration tests) |
| `cargo test` in `daemon/` | 65 tests pass (56 unit, 9 across the `no_payload_logging`, `protocol_golden` and `pty_roundtrip` integration tests) |
| `node --test frontend/tests/*.test.js` | 202 tests: 201 pass, 1 skipped, 0 fail |
| `python3 build/tools/test_toml_to_js.py` | 14 tests pass |
| `python3 build/tools/test_check_image_accounts.py` | 21 tests pass (the image account checker, run on a built `.wic` in the real build) |
| `node frontend/tests/e2e/menu-regression.mjs` | Headless Chromium against a mocked WebSocket; needs a playwright library and Chromium, and prints `SKIP` and exits 0 when either is missing (see `frontend/tests/README.md`) |

CI (`.github/workflows/ci.yml`) also runs `cargo clippy`, the config check and `node --check` over the frontend scripts.

## Not yet verified

No bitbake build of the account work has been run and nothing has run on a device. Open items: the package name `pam-plugin-faillock`, recipe parse and QA, flag passthrough, the faillock line ordering in `common-auth`, `/etc` overlay boot ordering and power-cut behaviour, the M press interception and arrow position on the real panel, key and password login as `kiosk`, and the armed factory reset. The full list is in [`users.md`](users.md#not-verified).

## Known limitations

- Chromium runs as root with `--no-sandbox`, `--single-process` and a loopback debugging port, see [`hardware.md`](hardware.md).
- Password SSH gives single-factor remote root through `kiosk` and `sudo`.
- If the `/etc` overlay fails, the device has no remote access (degraded mode).
