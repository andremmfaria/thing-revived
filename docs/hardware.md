# Hardware At a Glance

Full detail and evidence trail: [`research/hardware-verification.md`](research/hardware-verification.md) (live `adb` shell findings) and [`research.md`](research/research.md) §1/§8 (teardown-sourced background). This file is a quick-reference summary only: if a number here ever disagrees with those files, the research files are authoritative.

**Firmware update (2026-11-16): the table below describes stock firmware**, which is no longer what's flashed on the device as of tonight's reflash to `yocto-superbird` (see [`yocto-build-and-flash.md`](yocto-build-and-flash.md) and the REVISION note in [`dependency-surface.md`](dependency-surface.md)). The physical silicon facts (SoC, RAM, display, input nodes) are unchanged; everything OS-dependent is not:

| Property | Stock firmware (below) | yocto-superbird (now flashed) |
|---|---|---|
| Userspace ABI | 32-bit `armhf` | genuinely 64-bit, `aarch64-unknown-linux-gnu` |
| OS base / init | Buildroot 2019.02.1, `supervisord` | Yocto-built, systemd |
| Device access | `adb` over USB | SSH over USB CDC-NCM, no `adb` |
| On-device browser | Chromium 69 (2018) | a much newer Chromium kiosk build |

See [`yocto-build-and-flash.md`](yocto-build-and-flash.md) for the full build/flash workflow and exact confirmation commands for the new state.

| Property | Value |
|---|---|
| SoC | Amlogic G12A / S905 family |
| RAM | ~488MB total, ~323MB available in steady state (measured, full stock stack running) |
| Kernel | Linux 4.9.113, reports `aarch64` |
| Userspace ABI | **32-bit `armhf`** (ARMv7 hard-float) despite the 64-bit kernel; native builds must target `armv7-unknown-linux-gnueabihf` |
| OS base | Buildroot 2019.02.1 (OTA-patched), BusyBox v1.36.0 |
| GPU | Vendor Mali blob via `/dev/mali0` + `/dev/dri/card0` (not Panfrost) |
| Bluetooth | `hci0` live without any re-flash |
| Display | 4" 800×480 touchscreen, I2C controller (`tlsc6x`) |
| Input | `/dev/input/event0` = 5 physical buttons (`gpio-keys`), `/dev/input/event1` = knob rotation (`rotary@0`, `REL_HWHEEL` deltas, not discrete keys), `/dev/input/event3` = touchscreen |
| Current firmware | Stock Buildroot launcher/Chromium architecture (confirmed via CDP; not unmodified stock Spotify firmware as first assumed). The unit originally ran **DeskThing** as a community client, but that has since been fully overwritten by thing-revived's own UI, dashboard, and terminal webapps — no DeskThing code or config remains on the device; see [`dependency-surface.md`](dependency-surface.md) |
| On-device browser | Chromium 69 (2018); UI builds must target this explicitly (e.g. Vite `build.target: 'chrome69'`) |
| Access | `adb` over USB works out of the box (vendor-specific USB interface class matches the standard ADB signature); root shell |

See [`architecture.md`](architecture.md) for how these constraints shaped the system design.
