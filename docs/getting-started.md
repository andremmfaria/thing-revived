# Getting Started: Running the Prototype (historical stub)

## REVISION (2026-11-16): superseded by the yocto-superbird reflash

Everything below describes the pre-reflash, stock-firmware deployment workflow (`adb`, `armv7-unknown-linux-gnueabihf`, `supervisord`, Chromium 69); the device was reflashed to `yocto-superbird` on 2026-11-16, which replaced all four of those — see [`hardware.md`](hardware.md)'s before/after table for exactly what changed and [`yocto-build-and-flash.md`](yocto-build-and-flash.md) for the current build/customize/flash/deploy workflow. Kept below as a historical stub, not a reproducible procedure.

## What this workflow used to do

This doc originally gave full step-by-step `adb` commands for deploying thing-revived to the device's stock firmware: building and pushing a `controller/frontend/` React demo UI (dev-only, later removed from the repo entirely, never shipped) to its own on-demand path; building and pushing the Rust bridge daemon (`controller-agent`, statically linked for `armv7-unknown-linux-gnueabihf`) along with a `supervisord` program entry so it survived reboot; swapping which app occupied the kiosk's default boot path between this UI and the NOC dashboard; and a lighter dev-iteration loop serving the build over a local `busybox httpd` instead of pushing to a persistent path.

None of this is runnable against the device as currently flashed: `adb` doesn't exist on `yocto-superbird` (SSH over USB-CDC-NCM instead), the cross-compile target was 32-bit `armv7-unknown-linux-gnueabihf` (now `aarch64-unknown-linux-gnu`), and the `supervisord` entries are gone (systemd units instead).

For the current, real deployment workflow, see [`yocto-build-and-flash.md`](yocto-build-and-flash.md).

## Known open item (resolved)

Exact button/knob scancode-to-physical mapping was captured by the user through a live hardware test and confirmed working correctly; see [`research/hardware-interaction.md`](research/hardware-interaction.md) for the pipeline this feeds into.
