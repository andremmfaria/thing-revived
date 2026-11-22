# Coming Soon

A backlog of open items, checked against the repo. Nothing here is committed or scheduled; it is a menu to pick from, not a roadmap with dates. An item that gets picked up goes through a design pass before code lands, and moves into the current-state docs once it is fact.

Already shipped and therefore not listed: the Wi-Fi menu and USB Wi-Fi in host mode ([`wifi.md`](wifi.md)), USB Ethernet through the dock, Bluetooth keyboard input, NTP time sync (systemd-timesyncd), Terminal font size and font family settings, and automatic respawn of a crashed shell (up to 3 times in 30 s, then a manual respawn is offered).

## Terminal UX

Wedge detection: a shell that exits is respawned automatically within the daemon's limit, but nothing detects a PTY that hangs without exiting, so recovery from a hang is still a manual respawn from the UI.

Scrollback search: xterm.js ships a search addon, but only the fit addon is vendored and loaded ([`frontend/vendor/README.md`](../frontend/vendor/README.md)).

Copy and paste: there is no mouse on this device, so this needs design before implementation, likely touch-select plus a menu action rather than a conventional selection drag.

Session persistence across an agent restart: the daemon keeps nothing beyond its process lifetime, so restarting it kills every shell. The daemon proposal concludes this cannot live inside that process; the options are running `tmux` or `abduco` as the session command or splitting out a small session-host process, see [`research/architecture-proposals/daemon.md`](research/architecture-proposals/daemon.md). Shell state across a reboot is not realistic.

Per-tab activity indicator: some signal when output arrives in a background session.

Terminal input without a Bluetooth keyboard: the terminal view takes text only from a paired keyboard. The Chromium on-screen keyboard is used for the Wi-Fi password field, but nothing equivalent feeds PTY input.

## Settings menu

Color theme options: the Terminal menu offers font size and font family, but the colors come only from the `theme_*` keys in [`config/frontend.toml`](../config/frontend.toml) and cannot be changed on the device.

## Hardware and network verification

Only one USB Wi-Fi adapter has been tested on hardware (D-Link DWA-171, `rtw88_8821cu`). The other drivers in the image are untested.

WPA3-SAE, open networks, 5 GHz scanning without a country code and the exact wpa_supplicant events for failure cases are covered only by unit and fake-socket tests, see the Not verified section of [`wifi.md`](wifi.md).

Dock power behavior is undocumented and untested: how the device is powered while the port is in host mode behind a dock, and whether the dock's PD pass-through powers or charges it ([`hardware.md`](hardware.md)).

Bluetooth PAN as a general-purpose network link: Bluetooth is used only for keyboard pairing. USB Wi-Fi and dock Ethernet are the working network paths, so PAN would be an alternative requiring no extra hardware, not a gap.

## Operability

OTA updates: every change today means a physical burn-mode reflash. The stock firmware ran `swupdate` ([`research/hardware-verification.md`](research/hardware-verification.md)), and the BSP's update images have never been tried with this project.

A lightweight on-device health check (is the terminal agent alive, are sessions up, is the clock synced). The daemon already reports `clock_sync` status taken from `timedatectl`, with `offset_ms` always 0, but there is no health endpoint.

Settings backup: brightness and timezone live in one on-device JSON file (`/var/lib/thing-revived/settings.json`) with no copy anywhere durable. Bluetooth pairing state is held by BlueZ, not in that file.

Idle-screen clock or status overlay for when no session has recent activity. The status bar already shows a clock and agent connection state; a full idle screen does not exist.

Self-hosted CI runner for the image build: [`.github/workflows/ci.yml`](../.github/workflows/ci.yml) builds and tests the Rust crates, runs clippy and a Python test, but nothing builds the Yocto image. This needs explicit sign-off before touching the build host's persistent setup.

## Build and dependency housekeeping

Dependency mismatch: `controller/` pins `tungstenite` 0.24 while `daemon/` uses 0.30.

No vendor fetch script: the vendored xterm.js and FitAddon are refreshed by hand from the pinned npm releases, with a trailing sourceMappingURL comment stripped, and the vendored builds are not covered by the syntax guard.

The acorn parse layer of the frontend syntax guard is skipped whenever acorn cannot be resolved offline, since acorn is not a repo dependency ([`frontend/tests/README.md`](../frontend/tests/README.md)). The token scan, `node --check` and CSS scan still run.
