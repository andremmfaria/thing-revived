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

## `terminal/` is the main active project

A real, vendored xterm.js 5.3.0 + FitAddon 0.8.0 terminal UI (`terminal/frontend/`), backed by a
Rust PTY agent (`terminal/agent/`, binary `terminal-agent`), reachable from a Bluetooth-paired
keyboard plus the device's own buttons/knob for session chrome (next/prev session, interrupt,
clear, respawn). Full design and wire protocol: [`terminal.md`](terminal.md).

**Status, as of the 2026-11-16 reflash:** both `terminal-agent` and the buttons/knob bridge
daemon (`control-surface-agent`, see below) were packaged as systemd-managed services and baked
directly into the custom webapp that is now the kiosk's default boot page, including the
vendored xterm.js assets (see [`yocto-build-and-flash.md`](yocto-build-and-flash.md) §5a/§5b/§5c
for the exact recipe changes). This is the furthest along of the three deliverables on this
device: it is deployed, exercised, and screenshotted against the physical unit on real hardware
on 2026-11-15 (pre-reflash, stock firmware) — see [`terminal.md`](terminal.md)'s "What was
verified on real hardware (2026-11-15)" section for exactly what was tested and the two real bugs
found and fixed there. One blocker remains open: real BT-keyboard pairing was still blocked on
stock firmware pending a reflash; whether it now works under `yocto-superbird` has not yet been
re-verified against the physical unit in this repo's history and should not be assumed either
way.

## `prototype/control-surface/` — kept for history/reference, but its bridge daemon is now live

The original hardware-input-to-screen prototype: a Rust bridge daemon (`agent/`, binary
`control-surface-agent`, uses `evdev` for input) relaying raw button/knob input events to a React
UI (`frontend/`) over a local WebSocket. Per [`prototype/README.md`](../prototype/README.md) this
directory was moved out of top-level status once `terminal/` became the main active project — but
unlike a purely archival prototype, its daemon is one of the two packages actually shipped into
the current `yocto-superbird` kiosk image (see above), because `terminal/`'s own session-chrome
input depends on it running.

- **Bluetooth/smart-home-controller client: still not implemented.** `Cargo.toml` has no
  Bluetooth crate today; this was the original plan (see
  [`architecture.md`](architecture.md#chosen-design-chromium-kiosk--separate-native-bridge-daemon))
  but has not been built.
- Button/knob scancodes are confirmed live on real hardware: see
  [`research/button-knob-clock-design.md`](research/button-knob-clock-design.md) and
  [`research/hardware-verification.md`](research/hardware-verification.md).
- To build/run it directly (note: describes the pre-reflash stock-firmware workflow, see the
  REVISION note at the top of that file): [`getting-started.md`](getting-started.md).

## `prototype/dashboard/` — kept for history/reference, not yet migrated to the new firmware

A NOC-style dashboard showing the device's own CPU, memory, disk, and I/O, rendered as a
dark/high-contrast monitoring wall. Full detail: [`dashboard.md`](dashboard.md).

- The agent (`prototype/dashboard/agent/`, binary `noc-agent`) was rewritten from an earlier
  Python script to Rust. **The Rust rewrite has only been built and run on a desktop Linux host**
  (`x86_64-unknown-linux-gnu` and a cross-compiled `armv7-unknown-linux-gnueabihf` binary were both
  exercised there); it has not yet been pushed to or run on the physical device. The on-device
  verification recorded in `dashboard.md` (supervisord entry `RUNNING`, live metrics file
  updating across a real reboot, a CDP screenshot of the rendered dashboard) all predates the
  Rust rewrite and was performed against the deleted Python version (`noc-agent.py`) on stock
  firmware.
- The frontend (`prototype/dashboard/frontend/`) is unchanged by the Rust rewrite and was not
  itself re-verified after it.
- Unlike `terminal-agent` and `control-surface-agent`, `noc-agent` and its frontend were **not**
  among the packages added to the `yocto-superbird` kiosk image recipe (see
  [`yocto-build-and-flash.md`](yocto-build-and-flash.md) §5c's `IMAGE_INSTALL:append` line, which
  names only `thing-revived-terminal-agent` and `control-surface-agent`). Whether/when the
  dashboard gets ported to the new firmware is open; flagged here rather than assumed.

## Which project is "the main one"

`terminal/` is the main active project going forward. `prototype/control-surface/` and
`prototype/dashboard/` are earlier prototypes kept for history — but note `control-surface/`'s
bridge daemon is a live runtime dependency of `terminal/`'s session chrome, not purely archival,
while `prototype/dashboard/` has no such dependency and is the one piece of this project not yet
carried over to the new firmware.
