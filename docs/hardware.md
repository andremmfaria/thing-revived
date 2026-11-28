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
| Host | Wi-Fi and Ethernet adapters, USB mass storage, through a USB-C dock or OTG adapter | Opt-in: Device settings, USB mode, Mode (live, persisted) |
| Device (gadget) | SSH over USB CDC-NCM, used for development and the build/flash workflow. The gadget is bound by `thing-revived-usb-bind.service` even when adb is off | Boot default in this image |

The image boots in device mode unless the persisted Mode says host: a systemd drop-in (`10-thing-revived-default-device.conf`, installed by the `superbird-usb-role` `.bbappend`) runs `superbird-usb-role to-device` unless `/var/lib/superbird-usb-role/boot-role` contains `host`, in which case it runs `to-host --no-revert`. The drop-in no longer forces device mode on ADB builds. `boot-role` is now a mirror: the boot apply unit A rewrites it from `device-settings.json` at every boot, and a first boot with an ADB seed of 1 starts in device mode even if a legacy `boot-role` says host. Upstream's file cannot be shipped in the image because `/var` is the data partition created at first boot. A burn-mode flash does not wipe `/var` when the partition geometry is unchanged (checked across several flashes, spike P0.10; a flash that changes the geometry, such as the move to 770 MiB root slots without bandaid, is a full flash that wipes it, see [`development.md`](development.md#image-layout-and-baked-tools); a device flashed with the older 516 MiB root layout will lose `/var` on reflash), so the `/etc` overlay's upper layer, `boot-role` and `device-settings.json` all survive a reflash, see [The /etc overlay](users.md#the-etc-overlay). Only a factory reset wipes them, and the image id reseed resets adb and uart (see Device settings below). Consequence: with the default image the NCM link and SSH as `kiosk` over USB are available at boot (once the `kiosk` password has been set). Host mode for USB Wi-Fi, Ethernet or storage adapters is chosen on Device settings, USB mode (see [Device settings](#device-settings)). Running `superbird-usb-role persist host` by hand is overwritten at the next boot by the persisted Mode. See [`yocto-build-and-flash.md`](yocto-build-and-flash.md) for the flash workflow and the SSH-over-USB steps.

The dev link does not need adb. Upstream composes the gadget (CDC-NCM plus an FFS adb function) in `superbird-usb-gadget.service` and leaves the UDC bind to `adbd`. This image enables neither `android-tools-adbd.service` nor `thing-revived-usb-bind.service` at boot: the boot apply (Device settings below) starts one of them, adbd when the persisted ADB setting is on, usb-bind otherwise. They are a `Conflicts=` pair, so starting either one stops the other and hands the UDC back, which both the boot apply and the live toggle use. `thing-revived-usb-bind.service` removes the unused `ffs.adb` link from the gadget configuration (an incomplete FFS function makes the bind fail with EBUSY) and binds the UDC NCM-only, retrying for about 10 seconds. With adb on, `adbd` binds as upstream does. The NCM side (`uncm0`, a DHCP server and the `10.42.1.x` address from `/run/systemd/network/11-usb-ncm.network`) is written by the gadget script and does not depend on `ffs.adb`.

### Device settings

The main menu has one Device settings row. Its first row is Unlock (the kiosk password flow, see [`users.md`](users.md#device-settings-and-the-users-menu)). Every other row stays greyed, highlightable but not selectable, until the admin session is unlocked. The unlock lasts a fixed 30 minutes, then the rows grey out again. Unlocked, the rows are Users, Groups, Reboot device (hold the knob push to confirm, a tap only shows a hint) and USB mode, and the Unlock row reads Lock now.

The USB mode screen has four inline rows. Mode switches the port between Host and Device live. SSH, ADB and UART are on or off toggles. Every change is applied at once and persisted in `/var/lib/thing-revived/device-settings.json`, with no confirm prompt beyond the unlock. Choosing Host forces ADB off (persisted off) and greys the ADB row with a short note, because adb cannot run in host role. Going back to Device re-enables the row but leaves ADB off. SSH and UART are independent of the mode. A note on the screen warns that switching SSH off can lock out remote access if the USB link and UART are off too, and the kiosk terminal stays available.

Link outage when changing mode: the controller runs `superbird-usb-role to-host --no-revert` or `to-device`. In the Phase 0 spike the host side lost the USB link for about 19 s in host role and needed about 8 s more to recover after returning to device. Changing ADB drops the link only briefly (about 6 to 10 seconds in the earlier spike). An open `ssh` session over the NCM link needs reconnecting after any mode or ADB change. The kiosk frontend's own socket is loopback and is unaffected.

A persistent `DEBUG` badge appears on every screen except the first-boot password screen while adb or the UART root console is live. It is fed from the live state, which the frontend asks for at connect, so a persisted-on surface shows right after boot.

#### Persistence and seeding

The file is `/var/lib/thing-revived/device-settings.json` (version 1, keys `image_id`, `mode`, `ssh`, `adb`, `uart`), written atomically with mode 0600. `THING_REVIVED_ENABLE_ADB` and `THING_REVIVED_ENABLE_UART` are only the first-boot seed: the image bakes `/usr/share/thing-revived/device-seed.json` from the two flags and an image id, and when the settings file is missing the first boot copies those values (ssh seeds on). A legacy `/var/lib/superbird-usb-role/boot-role` of `host` is honoured for Mode on that first boot unless the ADB seed is 1.

Reseed rule: when the image id baked into the seed file differs from the id stored in the settings file, `adb` and `uart` reset to the seed values, while `ssh` and `mode` keep their persisted values. The id is updated in the same write, so it fires once per image. Because `/var` survives a burn-mode flash, this is what stops a persisted adb true from outliving a reflash to a hardened image.

Fail-safe defaults apply on any doubt. A corrupt or invalid file is renamed to `device-settings.json.corrupt` and replaced with device mode, ssh on, adb off, uart off. A file that says host with adb on is forced to adb off. If `/var` cannot be read or written the same defaults apply in memory and every change is refused with `persist_failed`.

#### Boot apply

Nothing about adbd, usb-bind or the UART console is enabled in the image any more. Three units from the `thing-revived-device-apply` recipe decide at every boot. All run `controller-agent apply-device-settings`, which never reads `controller.toml`, so a stale config cannot stop it.

- **Unit A, `thing-revived-device-settings.service`** (early, `DefaultDependencies=no`, `WantedBy=sysinit.target`, before `superbird-usb-role.service`): loads, seeds, reseeds or repairs the JSON, mirrors Mode to `boot-role`, and writes or clears the `/run/thing-revived/ssh-off` flag. It starts no unit.
- **Unit B, `thing-revived-device-apply.service`** (late, `WantedBy=multi-user.target`, after the gadget and role units): starts exactly one unit of each pair, adbd or usb-bind for the gadget side (neither in host mode), `thing-revived-uart-root@ttyAML0.service` or the stock `serial-getty@ttyAML0.service` for the console, and starts or stops `sshd.socket`. It must never be ordered `Before=` a unit it starts, or it would wait on units that wait for it.
- **Unit F, `thing-revived-device-failsafe.service`** (started only by `OnFailure=` of B): if B failed, starts usb-bind (device mode only) when no gadget unit is active and the stock getty when no console unit is active. It never changes the JSON and never starts adbd or the root UART, so a failed apply can leave a persisted adb or uart off, and the badge then shows the true live state.
- **sshd.socket gate**: the drop-in `sshd.socket.d/20-thing-revived-ssh-gate.conf` orders the socket after unit A and adds `ConditionPathExists=!/run/thing-revived/ssh-off`, so a persisted ssh off never opens even in the first seconds of boot. If unit A itself fails, ssh is left reachable on purpose.

The drop-in `10-thing-revived-default-device.conf` no longer reads `/etc/usb-debugging-enabled`. It reads only `boot-role`, which unit A has synced from the JSON. The CLI stays a fallback (`superbird-usb-role persist`, `to-host`, `to-device`), but the JSON is the source of truth and is re-mirrored at every boot. Wire messages: `device_settings_get`, `device_settings_set` and `device_reboot` with the events `device_settings`, `device_settings_error` and `device_rebooting`, see [`../config/protocol/controller/README.md`](../config/protocol/controller/README.md). The program names, unit names and file paths are the `[device_settings]` keys in [`../config/controller.toml`](../config/controller.toml).

Security consequences (persisted root adb and UART on any build, no auto-disable, factory reset as recovery) are stated in [`users.md`](users.md#security-notes-and-accepted-limits).

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
- Whether `thing-revived-usb-bind.service` binding (or failing to bind) in host role is harmless. The spike showed that starting `adbd` in host role succeeds and binds the UDC, but the new usb-bind unit was not exercised there, so the failsafe unit never starts it in host mode.

- The CDC-NCM dev link with adb off (`thing-revived-usb-bind.service`) has not been run on a device yet.
- Wi-Fi adapters other than the DWA-171 rev C1.
- The Device settings rework above is implemented and awaiting device verification. The Phase 0 spike (`research/device-settings-spikes.md`) verified the live ssh toggle and the sshd gate condition, the live host and device switch, the console getty generator behaviour and `/var` surviving burn-mode flashes. Not yet run on a device: the three boot units and their ordering, the persisted-file edge cases with the real binary, the live UART login, and the bitbake build.

See [`architecture.md`](architecture.md) for how these constraints shaped the system design, and [`yocto-build-and-flash.md`](yocto-build-and-flash.md) for the build/flash workflow.
