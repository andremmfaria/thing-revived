# Current State: What's Actually Implemented and Running

This is the single canonical account of what's actually built and running today, consolidated
from README.md's old "Layout" status callouts, `overview.md`'s old "Current state" section, and
`architecture.md`'s old "Current implementation status" section — those three now point here
instead of each independently restating (and drifting from) the same facts.

## Firmware: the device was reflashed to `yocto-superbird` on 2026-11-16

Everything below describes the device as it stands after that reflash, documented in full in
[`yocto-build-and-flash.md`](yocto-build-and-flash.md): `ssh` over USB-CDC-NCM (no more `adb`),
`aarch64-unknown-linux-gnu` userspace (no more 32-bit `armv7`), systemd-managed services (no more
`supervisord`), and a newer Chromium kiosk build. See [`hardware.md`](hardware.md) for the
before/after table and [`dependency-surface.md`](dependency-surface.md) for the REVISION note on
what that changes about the dependency story. Docs describing the pre-reflash, stock-firmware
workflow (`getting-started.md`, parts of `dashboard.md`) are explicitly marked as superseded
audit trail where that applies — read their own REVISION notes before following their exact
commands.

## Repo layout: flattened on 2026-11-21

The repo root now holds `daemon/` (the terminal's PTY-relay Rust agent, binary `terminal-agent`),
`frontend/` (the terminal's static HTML/CSS/JS UI), `controller/` (the buttons/knob bridge Rust
daemon, binary `controller-agent`), `deploy/` (the one surviving legacy supervisord conf, for
`thing-revived-terminal`, kept for historical/stock-firmware reference only), and `docs/`. The
earlier `terminal/agent` + `terminal/frontend` nesting and `controller/agent` nesting are gone —
each Rust component is now a plain crate at its own top-level directory, no intermediate `agent/`
subdirectory. `controller/frontend/` (a standalone React demo UI for the bridge daemon, never
shipped to the device) was deleted outright, not moved — it predates `terminal/` becoming the
main project and had no consumers left.

## `daemon/` + `frontend/` (the terminal) is the main active project

A real, vendored xterm.js 5.3.0 + FitAddon 0.8.0 terminal UI (`frontend/`), backed by a
Rust PTY agent (`daemon/`, binary `terminal-agent`), reachable from a Bluetooth-paired
keyboard plus the device's own buttons/knob for session chrome (next/prev session, interrupt,
clear, respawn). Full design and wire protocol: [`terminal.md`](terminal.md).

**Status, as of the 2026-11-21 rebuild/reflash:** both `terminal-agent` and the buttons/knob
bridge daemon `controller-agent` (see below) run as systemd-managed services baked directly into
the custom webapp that is the kiosk's default boot page, including the vendored xterm.js assets
(see [`yocto-build-and-flash.md`](yocto-build-and-flash.md) §5a/§5b/§5c for the exact recipe
changes). This is confirmed live on the physical unit, not just built: `terminal-agent.service`
and `controller-agent.service` both running, 4 live terminal sessions, both WebSocket ports
(8765 bridge, 8766 terminal) listening with active connections from the kiosk's own Chromium
page. Real BT-keyboard pairing — the one blocker still open as of the 2026-11-15 pre-reflash
hardware session (blocked by stock firmware's `bluetoothd` plugin whitelist) — is now directly
confirmed working: paired, and typing from a real BT keyboard into all 4 live terminal sessions,
verified on 2026-11-21 against the `yocto-superbird`-flashed device. See the REVISION note at the
top of [`terminal.md`](terminal.md) for the full picture of what changed since the original
2026-11-15 hardware session.

## `controller/` — the buttons/knob bridge daemon, live and current

A Rust daemon (`controller/`, binary `controller-agent`, uses `evdev` for input) relaying
button/knob input events to `frontend/`'s session-chrome handler over a local WebSocket, plus the
settings-menu backend (Bluetooth, brightness, timezone). Renamed from the original
`prototype/control-surface/` (history preserved via `git mv`, crate/binary renamed from
`superbird-bridge`) on 2026-11-21, then flattened from `controller/agent/` to `controller/`
directly. It is one of the two packages actually shipped into the current `yocto-superbird`
kiosk image, because `frontend/`'s own session-chrome input depends on it running — not a
prototype kept for reference, a live runtime dependency. The yocto BSP recipe and systemd unit
were renamed to match (`controller-agent` / `controller-agent.service`) and the image rebuilt and
reflashed on 2026-11-21; this is confirmed running on the physical device under the new name, not
just committed source.

- **Bluetooth/smart-home-controller client: out of scope, dropped.** `Cargo.toml` has no
  Bluetooth-peripheral/smart-home crate and none is planned — this was the project's original
  premise (see
  [`architecture.md`](architecture.md#chosen-design-chromium-kiosk--separate-native-bridge-daemon))
  but was explicitly cut from scope in favor of the terminal becoming the main deliverable.
- Button/knob scancodes, the `EVIOCGRAB`+`uinput` session-chrome mechanism, and the
  Bluetooth/brightness/timezone settings-menu backend are all confirmed live on real hardware:
  see [`research/button-knob-clock-design.md`](research/button-knob-clock-design.md) and
  [`research/hardware-verification.md`](research/hardware-verification.md).
- `controller/frontend/` (the standalone React demo UI) no longer exists — see "Repo layout"
  above.

## `prototype/dashboard/` — removed entirely

See [`dashboard.md`](dashboard.md) for the removal note; its former implementation history is
retained in git history only.

## Which project is "the main one"

The terminal (`daemon/` + `frontend/`) is the main active project going forward. `controller/` is
not a prototype — it's a live runtime dependency of the terminal's session chrome and settings
menu, shipped in the same kiosk image under its own CI and its own renamed recipe/unit. The
former `prototype/dashboard/` had no such dependency and has been removed entirely, with no
replacement.
