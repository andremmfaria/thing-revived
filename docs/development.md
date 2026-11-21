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

## `controller/` — kept for history/reference, but its bridge daemon is now live

The original hardware-input-to-screen prototype, moved from `prototype/control-surface/` to
`controller/` at the repo root (history preserved via `git mv`): a Rust bridge daemon (`agent/`,
crate/binary renamed from `superbird-bridge` to `controller-agent`, uses `evdev` for input)
relaying raw button/knob input events to a React UI (`frontend/`) over a local WebSocket. Unlike a
purely archival prototype, its daemon is one of the two packages actually shipped into the current
`yocto-superbird` kiosk image (see above), because `terminal/`'s own session-chrome input depends
on it running. The currently-flashed image still ships the daemon under its pre-rename yocto
recipe/unit names, `control-surface-agent` (recipe) and `control-surface-agent.service` (systemd
unit) — those will become `controller-agent` and `controller-agent.service` once the BSP recipe is
rebuilt against this renamed source in a later step; that rebuild has not happened yet.

- **Bluetooth/smart-home-controller client: still not implemented.** `Cargo.toml` has no
  Bluetooth crate today; this was the original plan (see
  [`architecture.md`](architecture.md#chosen-design-chromium-kiosk--separate-native-bridge-daemon))
  but has not been built.
- Button/knob scancodes are confirmed live on real hardware: see
  [`research/button-knob-clock-design.md`](research/button-knob-clock-design.md) and
  [`research/hardware-verification.md`](research/hardware-verification.md).
- To build/run it directly (note: describes the pre-reflash stock-firmware workflow, see the
  REVISION note at the top of that file): [`getting-started.md`](getting-started.md).

## `prototype/dashboard/` — removed entirely

The NOC-style dashboard showing the device's own CPU, memory, disk, and I/O (rendered as a
dark/high-contrast monitoring wall) has been removed from this repository entirely: it was never
deployed to the physical device, was never among the packages added to the `yocto-superbird` kiosk
image recipe, and has no replacement. See [`dashboard.md`](dashboard.md) for the removal note; its
former implementation history is retained in git history only.

## Which project is "the main one"

`terminal/` is the main active project going forward. `controller/` is an earlier prototype kept
for history — but note its bridge daemon is a live runtime dependency of `terminal/`'s session
chrome, not purely archival. The former `prototype/dashboard/` had no such dependency and has been
removed entirely, with no replacement.
