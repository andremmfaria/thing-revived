# Coming Soon

A backlog of ideas for where this project could go next. Nothing here is committed or scheduled; it's a menu to pick from, not a roadmap with dates. Once an item gets picked up for real, it goes through a proper design pass before any code lands, and the doc gets updated or the item moves into the actual docs once it's current-state fact rather than an idea.

## Terminal UX

Auto-respawn/wedge detection: right now a hung PTY needs a manual button-triggered respawn; nothing detects a hang on its own and recovers automatically.

Scrollback search: xterm.js ships a search addon that isn't wired in yet.

Copy/paste: there's no mouse on this device, so this needs real design thought before implementation, likely touch-select plus a menu action rather than a conventional selection drag.

Session persistence across reboot/respawn: a tmux-style reattach instead of losing shells whenever the agent restarts or the device reboots.

Per-tab activity indicator: some signal when output arrives in a background session you're not currently looking at.

On-screen keyboard fallback via touch plus the knob: the terminal's text input currently has a hard dependency on a paired BT keyboard; a fallback input method would make it usable standalone.

## Controller / settings menu

Font size and color theme options in the settings menu.

SSH client confirmation or install on-device, so the existing `ssh-devel` session config in the terminal actually works end to end (currently unconfirmed, likely missing on the flashed image).

## Operability

OTA updates via the BSP's own `swupdate` delta mechanism. Every change today means a physical burn-mode reflash, which needs hands-on access to the device; the BSP already ships `superbird-kiosk-update-{dev,prod}` images and delta packages that have never been used.

A lightweight on-device health/status check (is `terminal-agent` alive, are sessions up, when did the clock last sync) reachable from the usual dev host over the existing link. Not a dashboard revival, just a lean health endpoint.

Settings backup: brightness, timezone, and BT-pairing state all live in one on-device JSON file with no backup anywhere. Worth mirroring it somewhere durable.

A self-hosted GitHub Actions runner on the build host, so the yocto image build can run from CI instead of only by hand. Needs explicit sign-off before touching that host's persistent setup.

## Bigger ideas

An idle-screen clock/status overlay for when no terminal session has recent activity. The wire protocol already has `clock_sync` plumbing that could feed this.

Bluetooth PAN as a real general-purpose network link. BT today is only used for keyboard pairing; using it for actual data networking was the project's own original research recommendation and is still unused. The device has no on-board WiFi, so BT PAN is the only wireless networking path that needs no extra hardware (USB Wi-Fi adapters are supported in host mode, see `docs/wifi.md`).
