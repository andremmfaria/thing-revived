# Hardware At a Glance

Full detail and evidence trail: [`research/hardware-verification.md`](research/hardware-verification.md) (live shell findings) and [`research/research.md`](research/research.md) §1/§8 (teardown-sourced background). This file is a quick-reference summary only: if a number here ever disagrees with those files, the research files are authoritative.

## Device

| Property | Value |
|---|---|
| SoC | Amlogic S905D2 (G12A family) |
| GPU | Mali-G31 MP2 (Bifrost), Mesa/Panfrost acceleration |
| RAM | ~488MB total (512MB nominal) |
| Display | 4" 800×480 touchscreen, I2C controller (`tlsc6x`) |
| Input | `/dev/input/event0` = `gpio-keys`, 7 keycodes: `KEY_ESC`=1, `KEY_1`..`KEY_4`=2..5, `KEY_ENTER`=28 (knob-push), `KEY_M`=50 (see [`research/button-knob-clock-design.md`](research/button-knob-clock-design.md)); `/dev/input/event1` = knob rotation (`rotary@0`, `REL_HWHEEL` deltas, not discrete keys); `/dev/input/event3` = touchscreen |
| OS / init | `yocto-superbird`, systemd-managed services |
| Userspace ABI | `aarch64-unknown-linux-gnu` |
| On-device browser | Chromium 147 kiosk (verified on the device on 2026-11-22: V8 14.7.173.20, binary string Chrome/147.0.7727.116). The frontend still follows a conservative baseline from the earlier Chromium 68 or 69 assumption (classic scripts, no flexbox `gap`, `inset`, `clamp` or `aspect-ratio`), enforced by `frontend/tests/syntax-guard.test.js`. The browser no longer requires it; see the [audit](research/chromium-147-frontend-audit.md) |
| Accounts and access | `root` locked, `kiosk` the administrator (password or SSH key, `sudo`), first-boot password set with the M button, see [`users.md`](users.md). The USB-C port boots in device mode, so the CDC-NCM link and SSH as `kiosk` exist by default |
| Network exposure | `controller-agent` and `terminal-agent` listen on loopback only (`127.0.0.1:8765` and `127.0.0.1:8766`, set in `config/controller.toml` and `config/terminal.toml`), see the Security model in [`architecture.md`](architecture.md#security-model) |

## Browser security limitation

The kiosk Chromium runs as root with `--no-sandbox`, and it exposes a remote debugging port on loopback (`127.0.0.1:9223`, with `--remote-allow-origins=*`). It also runs with `--single-process`. This is a known security limitation of the current image and is not fixed yet: anything able to reach the loopback port can drive a root browser, and a renderer compromise is not contained by a sandbox. Verified on the device on 2026-11-22. The same applies to the first-boot password screen, which is readable by any local process through that port while it is up (see [`users.md`](users.md#security-notes-and-accepted-limits)).

## USB-C port and roles

The single USB-C port runs in one of two roles, switched by the BSP's `superbird-usb-role` tool (`to-host`, `to-device`, `persist host`, `persist device`, `persist clear`).

| Role | What it gives | Status |
|---|---|---|
| Host | Wi-Fi and Ethernet adapters, USB mass storage, through a USB-C dock or OTG adapter | Opt-in: `superbird-usb-role persist host`, effective from the next boot |
| Device (gadget) | SSH over USB CDC-NCM, used for development and the build/flash workflow. The gadget is bound by `thing-revived-usb-bind.service` even when adb is off | Boot default in this image |

The image boots in device mode: a systemd drop-in (`10-thing-revived-default-device.conf`, installed by the `superbird-usb-role` `.bbappend`) runs `superbird-usb-role to-device` unless `/var/lib/superbird-usb-role/boot-role` contains `host`, in which case it runs `to-host --no-revert`. Builds whose boot default is `THING_REVIVED_ENABLE_ADB=1` (which ship `/etc/usb-debugging-enabled` as a build-time-only marker, never read or written at runtime) force device mode regardless of a persisted host role, so the dev link always exists; builds whose boot default is `0` honour a persisted host role. This is separate from the runtime adb toggle below: the flag only decides what the port and adbd look like right after boot. Upstream's file cannot be shipped in the image because `/var` is the data partition created at first boot. The `/etc` overlay's upper layer is on the same partition and also survives a reflash, see [The /etc overlay](users.md#the-etc-overlay). Consequence: with the default image the NCM link and SSH as `kiosk` over USB are available at boot (once the `kiosk` password has been set). Host mode for USB Wi-Fi, Ethernet or storage adapters is opt-in: choose Host in Settings, USB mode (see [USB mode in the Settings menu](#usb-mode-in-the-settings-menu)), or run `superbird-usb-role persist host` on the device, and reboot, it takes effect from the next boot. `superbird-usb-role persist device` or `persist clear` returns to the default. `to-host` and `to-device` switch the current boot only. Reflashing through burn mode also works. See [`yocto-build-and-flash.md`](yocto-build-and-flash.md) for the flash workflow and the SSH-over-USB steps.

The dev link does not need adb. Upstream composes the gadget (CDC-NCM plus an FFS adb function) in `superbird-usb-gadget.service` and leaves the UDC bind to `adbd`, which this image keeps off at boot unless its boot default is `THING_REVIVED_ENABLE_ADB=1`. `android-tools-adbd.service` and `thing-revived-usb-bind.service` (the `thing-revived-usb-bind` recipe) are a `Conflicts=` pair: starting either one stops the other and hands the UDC back, which is what both the boot-time default and the runtime toggle below use. At boot, whichever side is enabled wins: `thing-revived-usb-bind.service` removes the unused `ffs.adb` link from the gadget configuration (an incomplete FFS function makes the bind fail with EBUSY) and binds the UDC NCM-only, retrying for about 10 seconds; with the boot default at `1` the file exists, `thing-revived-usb-bind.service` is disabled instead, and `adbd` binds as upstream does. The NCM side (`uncm0`, a DHCP server and the `10.42.1.x` address from `/run/systemd/network/11-usb-ncm.network`) is written by the gadget script and does not depend on `ffs.adb`.

### USB mode in the Settings menu

Settings has a USB mode row that reads `USB mode: Device` or `USB mode: Host` (the role the port is in now, plus `(Host next boot)` or `(Device next boot)` when the saved choice differs). It is always listed, including when no Wi-Fi adapter exists, because enabling host mode is how an adapter gets used at all. Selecting it opens a screen with the running role, the role saved for the next boot, `Boot as Device` and `Boot as Host`, and a `Reboot now` row.

Choosing a role only persists it (the controller runs `superbird-usb-role persist host|device`) and it applies from the next boot. The menu never runs `to-host` or `to-device`, so nothing switches under the user and the USB link they may be using stays up until the reboot. `Reboot now` is a hold-to-confirm row like the destructive Users rows: hold the knob push; a tap or an ESC tap only shows a hint. The controller then runs `systemctl reboot`.

Warning, also printed on the screen: after the next boot in Host mode the USB link to a PC (ssh and adb over the cable) is gone. Recovery is `superbird-usb-role persist device` from the on-device terminal (a Bluetooth keyboard or a dock Ethernet or Wi-Fi SSH session) followed by a reboot, or a reflash through burn mode; `/var` survives a reflash, so a persisted `host` does too.

On builds whose boot default is `THING_REVIVED_ENABLE_ADB=1` (`/etc/usb-debugging-enabled` exists) the image forces device mode at every boot, so the screen shows `Next boot: Device (forced)` and a note that the saved choice is ignored. The controller reports this as `adb_forced` in the `usb_role` event.

The CLI stays the fallback and the only way to reach `persist clear`, `to-host` and `to-device`. Wire messages: `usb_role_get`, `usb_role_set` and `usb_role_reboot` with the events `usb_role`, `usb_role_error` and `usb_role_rebooting`, see [`../config/protocol/controller/README.md`](../config/protocol/controller/README.md). The controller runs as root (its unit sets no `User=`), which `persist` and the restart need; the program names and the ADB flag path are the `[usb_role]` keys in [`../config/controller.toml`](../config/controller.toml).

### Debug access (adb and UART) toggle

Settings, Debug access, behind the Users admin unlock, offers an independent on-the-fly toggle for adb and for the UART root console, on every build including prod. `THING_REVIVED_ENABLE_ADB` and `THING_REVIVED_ENABLE_UART` only decide the state each surface starts in at boot (`1` starts enabled, `0`, the default, starts disabled); the runtime toggle works regardless of either flag's value and is volatile, so a reboot always returns to whichever default the build shipped with. Enabling adb needs the USB role to be Device first; the screen names this and points at USB mode when it is not.

Operator procedure: open Settings, Users and unlock with the kiosk password, then Settings, Debug access. Hold the row for the surface you want (`Enable ADB` or `Enable UART console`) until it confirms; tapping alone does nothing. Expect the USB link to drop for roughly 6 to 10 seconds during the switch, the UI says so before the hold completes; an open `ssh` session over the NCM link needs reconnecting, adb itself comes back on its own. Confirm adb is on by running `adb devices` from a host and seeing the device offer a shell; confirm UART is on by opening the serial console and seeing a root prompt with no password, instead of the normal login prompt. A persistent `DEBUG` badge is visible on every other screen while either surface is on. Turn a surface off the same way: hold the matching `Disable` row, or just lock or let the admin session expire, since either surface enabled at runtime auto-reverts to its build default when that session ends. A reboot also always returns both surfaces to the build default.

## Networking over the dock

The device has no on-board Wi-Fi or Ethernet. Both come from USB devices in host mode.

- **Wi-Fi**: verified on real hardware on 2026-11-22 with a D-Link DWA-171 (rev C1, driver `rtw88_8821cu`) behind a USB-C dock. The image also carries drivers for DWA-171 rev A1 (`rtw88_8821au`) and B1 (`mt76x0u`), other rtw88 USB chips, `mt76x2u`, `mt7601u`, `rt2800usb` and `rtl8xxxu`; only the C1 has been tested. Design and menu behavior are in [`wifi.md`](wifi.md).
- **Ethernet**: a dock's wired Ethernet was also verified working on 2026-11-22. The image includes the kernel modules `usbnet`, `mii`, `r8152`, `ax88179_178a`, `asix`, `cdc_ether`, `cdc_ncm` and `rndis_host`. Which specific dock model and Ethernet chip was used is not recorded in the repo. [`research/lmp-mini-dock-chipsets.md`](research/lmp-mini-dock-chipsets.md) covers chipset candidates for one dock model and states its chips were never identified.
- **Route priority**: wired DHCP uses route metric 10 and Wi-Fi uses 20, so Ethernet keeps the default route when both are up.
- **Storage**: `usb-storage`, `uas` and `sd_mod` are in the image for USB mass storage in host mode. Verified on hardware on 2026-11-22: a USB mass storage device works through the dock.

## Peripherals

Bluetooth keyboards pair from the on-device Settings menu and appear as a normal evdev input device (the kernel enables `CONFIG_UHID` for BlueZ's input plugin). The terminal screen is usable with one. The Bluetooth pairing workflow is described in [`research/bt-peripheral-pairing-workflow.md`](research/bt-peripheral-pairing-workflow.md).

## Not verified

- Dock functions other than Ethernet, a Wi-Fi adapter and USB mass storage (card readers, HDMI). Which PD wattage the device needs behind a dock is not characterised; only that it is powered through the dock was confirmed (2026-11-22).
- The M button position relative to the panel (the first-boot arrow is a CSS constant), and the first-boot password screen on the real display.
- Whether `adbd` trying to bind the gadget controller while the port is in host role is harmless, and whether `thing-revived-usb-bind.service` binding (or failing to bind) in host role is harmless.

- The CDC-NCM dev link with adb off (`thing-revived-usb-bind.service`) has not been run on a device yet.
- Wi-Fi adapters other than the DWA-171 rev C1.
- The runtime debug access toggle above: the Phase 0 spike exercised stopping and starting the adb/usb-bind pair, relinking `ffs.adb`, the UART `Conflicts=` pair and the link-drop timing on a dev device (`research/debug-toggles-spikes.md`), but the feature itself has not been through a bitbake build or a flash.

See [`architecture.md`](architecture.md) for how these constraints shaped the system design, and [`yocto-build-and-flash.md`](yocto-build-and-flash.md) for the build/flash workflow.
