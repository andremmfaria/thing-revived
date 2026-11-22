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
| On-device browser | Chromium 69 kiosk (owner-confirmed). The frontend is written to an ES2018 subset with no CSS newer than Chromium 69 (no flexbox `gap`, `inset`, `clamp`, `aspect-ratio`), enforced by `frontend/tests/syntax-guard.test.js` |
| Network exposure | `controller-agent` and `terminal-agent` listen on loopback only (`127.0.0.1:8765` and `127.0.0.1:8766`, set in `config/controller.toml` and `config/terminal.toml`), see the Security model in [`architecture.md`](architecture.md#security-model) |

## USB-C port and roles

The single USB-C port runs in one of two roles, switched by the BSP's `superbird-usb-role` tool (`to-host`, `to-device`, `persist device`, `persist clear`).

| Role | What it gives | Status |
|---|---|---|
| Host | Wi-Fi and Ethernet adapters, USB mass storage, through a USB-C dock or OTG adapter | Boot default in this image |
| Device (gadget) | SSH over USB CDC-NCM, used for development and the build/flash workflow | Down at boot unless the device role is persisted |

The image boots in host mode: a systemd drop-in (`10-thing-revived-default-host.conf`, installed by the `superbird-usb-role` `.bbappend`) runs `superbird-usb-role to-host --no-revert` unless `/var/lib/superbird-usb-role/boot-role` contains `device`. Upstream's file cannot be shipped in the image because `/var` is the data partition created at first boot. Consequence: with the default image there is no SSH over USB at boot, and access is the on-device terminal (with a Bluetooth keyboard). To get the USB link back, run `superbird-usb-role to-device` for the current boot or `superbird-usb-role persist device` and reboot, from that terminal. Reflashing through burn mode also works. See [`yocto-build-and-flash.md`](yocto-build-and-flash.md) for the flash workflow and the SSH-over-USB steps.

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
- Whether `adbd` trying to bind the gadget controller while the port is in host role is harmless.
- Wi-Fi adapters other than the DWA-171 rev C1.

See [`architecture.md`](architecture.md) for how these constraints shaped the system design, and [`yocto-build-and-flash.md`](yocto-build-and-flash.md) for the build/flash workflow.
