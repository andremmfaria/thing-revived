# Dependency Surface

What thing-revived depends on to build and run, today. Crate versions below are the `Cargo.toml` requirements; the resolved versions are in each crate's `Cargo.lock`.

## Hardware and base image

1. **The physical device**: the Amlogic-based Spotify Car Thing ("superbird"): its SoC, the evdev input nodes (buttons and rotary knob), the sysfs backlight, the Bluetooth chip and the USB-C port. Fixed by the hardware, not something software choices can reduce.
2. **The `yocto-superbird` BSP**: the base Yocto system image, its `meta-superbird` layer (kernel, bootloader, device tree, `superbird-usb-role`) and the `meta-superbird-kiosk-example` layer that this project customizes through `build/overlay/` and builds with `kas-container`. See [`yocto-build-and-flash.md`](yocto-build-and-flash.md). systemd is the init system, and the userspace target is `aarch64-unknown-linux-gnu`.
3. **The on-device Chromium kiosk**: provided by the BSP image, Chromium 147 (verified on the device on 2026-11-22). It loads the project's webapp as the kiosk default page. The frontend code still follows an older, more conservative baseline that the browser no longer requires (see Frontend below and the [Chromium 147 frontend audit](research/chromium-147-frontend-audit.md)).
4. **BlueZ, systemd and wpa_supplicant on the device**: the controller talks to BlueZ over D-Bus, shells out to `timedatectl` and `systemctl`, and speaks the wpa_supplicant control socket. These come from the base image, not from this repo.

## Host build tooling

`build/build-image.sh` needs `cargo` with the `aarch64-unknown-linux-gnu` target, `gcc-aarch64-linux-gnu`, git, rsync, Python 3.11+ and `kas-container` (which needs Docker), plus 50-100GB+ of disk. The two Rust binaries are built with `+crt-static`. Details are in [`../build/README.md`](../build/README.md).

## Rust crates

Both crates are plain synchronous Rust with no async runtime, and both build with `opt-level = "z"`, LTO and stripping.

`controller/` (package `controller-agent`):

| Crate | Use |
|---|---|
| `evdev` 0.12 | Reading and grabbing the button and knob input devices |
| `tungstenite` 0.24 | Local WebSocket server (port 127.0.0.1) |
| `serde`, `serde_json` | Message encoding |
| `toml` 1 (no default features) | Config parsing (`controller.toml`) |
| `log`, `env_logger` 0.11 (no default features) | Logging to stderr, captured by journald |
| `zbus` 4 (default features, blocking API) | BlueZ D-Bus client and agent for the Bluetooth settings |

Wi-Fi control, brightness and timezone add no crates: the Wi-Fi flows use `std::os::unix::net::UnixDatagram` against the wpa_supplicant control socket (see [`wifi.md`](wifi.md)), brightness is sysfs, and timezone calls `timedatectl`.

`daemon/` (package `terminal-agent`):

| Crate | Use |
|---|---|
| `tungstenite` 0.30 | Local WebSocket server |
| `serde`, `serde_json` | Message encoding |
| `libc` 0.2 | `openpty`, `ioctl` and signal calls for PTY sessions |
| `toml` 1 (no default features) | Config parsing (`terminal.toml`) |
| `log`, `env_logger` 0.11 (no default features) | Logging to stderr |

The two crates intentionally share the same `toml`, `log` and `env_logger` pins and features. They differ on `tungstenite` (0.24 versus 0.30).

## Frontend

`frontend/` is a classic-script web app with no build step and no npm runtime dependency. The only third-party code is vendored under `frontend/vendor/`: xterm.js 5.3.0 (UMD build, `xterm.js` and `xterm.css`) and the xterm FitAddon 0.8.0 (`xterm-addon-fit.js`). Provenance and the reasons for UMD builds are in `frontend/vendor/README.md`. The frontend uses classic scripts and avoids newer syntax as a conservative baseline written when the browser was believed to be Chromium 68 or 69. Chromium 147 does not require that baseline. ES modules over `file://` are expected to be blocked, which is not yet tested on the device, so vendored libraries stay UMD builds and should be verified on hardware before an upgrade. Details: [Chromium 147 frontend audit](research/chromium-147-frontend-audit.md). `frontend/config.generated.js` is generated from `config/frontend.toml` by the image build.

## Image recipes under `build/overlay/`

- **Services**: `controller-agent` and `thing-revived-terminal-agent` install the prebuilt static binaries, their TOML config under `/etc/thing-revived/` and a systemd unit each. `superbird-kiosk-default-webapp` installs `frontend/` as the kiosk's default page.
- **Image contents** (`superbird-kiosk-prod-image.bbappend`): adds `bash`, the two agents, `thing-revived-ntp-config`, USB Ethernet and mass-storage kernel modules for host mode, and the USB Wi-Fi stack.
- **USB role default**: a `superbird-usb-role` drop-in (`10-thing-revived-default-device.conf`) makes the USB-C port boot in device role by default, so the NCM dev link and SSH as `kiosk` exist. Host role, which USB Wi-Fi, Ethernet and storage adapters through a dock need, is opt-in with `superbird-usb-role persist host` and takes effect from the next boot.
- **Wi-Fi**: kernel fragment `thing-revived-wifi.cfg` re-enables cfg80211 and mac80211 and builds Realtek rtw88, MediaTek mt76 and Ralink rt2800usb USB drivers as modules. The image adds the matching firmware packages (with a one-file `mt7610u` firmware package split out of `linux-firmware`), `wpa-supplicant`, `iw` and `wireless-regdb-static`. `thing-revived-wifi-config` provides a per-interface `thing-revived-wpa@.service`, a udev rule that starts it, a networkd DHCP file and a tmpfiles entry. `thing-revived-wifi-udev` ejects Realtek `0bda:1a2b` driver-CD-mode dongles through `usb-modeswitch`, whose `tcl` and data dependencies are dropped. Verified on hardware with a D-Link DWA-171 (rtw88_8821cu) through a USB-C dock. Dock Ethernet also works.
- **Accounts**: `thing-revived-accounts` (extrausers seed for `kiosk`, root lock, `/etc` overlay on `/var`, sshd drop-in, provision unit), the `sudo` bbappend, the `thing_revived_pam` image post-process (PAM edits live in the image, not in a libpam bbappend, to keep the libpam, libcap, systemd and chromium signatures unchanged), and the `pam-plugin-faillock` package (name unverified until the first build). The controller shells out to `useradd`, `userdel`, `chpasswd`, `gpasswd`, `ssh-keygen` and `unix_chkpwd`. See [`users.md`](users.md).
- **Time sync**: the device has no RTC. `thing-revived-ntp-config` installs a `timesyncd.conf.d` drop-in (pool servers, fallback servers, a 10 s connection retry) and explicitly enables `systemd-timesyncd`, because the image preset is `disable *`. A `superbird-clock` bbappend removes the upstream mask of `systemd-timesyncd`; `superbird-clock` itself stays and still bumps the clock forward to its saved or build floor early at boot. timesyncd follows systemd-networkd, so the clock syncs once any link (Wi-Fi or USB Ethernet) is online.
- **Bluetooth HID**: kernel fragment `thing-revived-uhid.cfg` enables `CONFIG_UHID` so BlueZ can turn paired keyboards into real input devices.

## What this project is not tied to

- **No Qt/QML runtime**: the Chromium-kiosk path was chosen over native Qt/QML; see [`research/architecture-decision.md`](research/architecture-decision.md).
- **No async runtime or web framework**: both Rust crates use blocking I/O and threads.
- **No npm or bundler**: the frontend ships as static files.
- **No smart-home or Bluetooth-peripheral controller client**: see the Non-goals section of [`overview.md`](overview.md).
