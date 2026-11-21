# Dependency Surface

What thing-revived actually depends on to run, today.

## What the system is actually tied to

1. **The physical SoC and hardware**: Amlogic G12A/S905D2, the evdev input nodes, the vendor Mali GPU, the Bluetooth chip. Fixed by the hardware itself, not something software choices can reduce.
2. **The `yocto-superbird` BSP**: the base system image, its `meta-superbird` layer (kernel, bootloader, device tree) and the `examples/meta-superbird-kiosk-example/` layer this project customizes via `build/overlay/` (see [`yocto-build-and-flash.md`](yocto-build-and-flash.md)). systemd is the init system; SSH over USB-CDC-NCM is the access path; the userspace ABI is `aarch64-unknown-linux-gnu`.
3. **The on-device Chromium kiosk**: provided by the BSP image, serving the project's own webapp (`frontend/`, vendored xterm.js + FitAddon) as the kiosk's default page.
4. **`controller/`'s Rust crates** (`controller-agent`, per `controller/Cargo.toml`): `evdev` for input, `tungstenite` for the local WebSocket server, `serde`/`serde_json` for message encoding, `zbus` for the BlueZ D-Bus settings-menu backend (Bluetooth, brightness, timezone).
5. **`daemon/`'s Rust crates** (`terminal-agent`, per `daemon/Cargo.toml`): `tungstenite` for the local WebSocket server, `serde`/`serde_json` for message encoding, `libc` for `openpty`/`ioctl`/session-leader PTY setup.
6. **`frontend/`'s vendored assets**: a UMD build of xterm.js 5.3.0 and the FitAddon 0.8.0 extension, shipped under `frontend/vendor/` (see `frontend/vendor/README.md` for provenance).

## What this project is not tied to

- **No Bluetooth-peripheral/smart-home-controller client dependency**: `controller/Cargo.toml` carries no such crate, and none is planned (see [`overview.md`](overview.md)'s Non-goals section).
- **No Qt/QML runtime**: the Chromium-kiosk path was chosen over native Qt/QML; see [`research/architecture-decision.md`](research/architecture-decision.md) for the full comparison, including what a Qt/QML fallback would cost if ever revisited.
- **No DeskThing code or configuration**: the device's prior community-client content was fully overwritten; see [`research/hardware-verification.md`](research/hardware-verification.md) for the verification history.
