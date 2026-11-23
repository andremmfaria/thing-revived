# Building, Customizing, and Flashing yocto-superbird

The device runs [`yocto-superbird`](https://github.com/JoeyEamigh/yocto-superbird), a community-maintained Yocto BSP replacement for the Spotify Car Thing. See [`dependency-surface.md`](dependency-surface.md) for the dependency picture this gives the project, and [`hardware.md`](hardware.md) for the resulting device facts, including the USB-C roles and docks.

## The build/flash scripts

`build/build-image.sh` and `build/flash-image.sh` automate the whole workflow; see [`build/README.md`](../build/README.md) for exact usage, flags and environment variables. Nothing from upstream is vendored or committed here: `build/overlay/` holds only this project's own small recipe, service, config and kernel-fragment text files, laid out to mirror their destination paths inside upstream's `examples/meta-superbird-kiosk-example/` layer (the layer upstream's own `kas/example-kiosk.yml` already includes, so no new layer is created). The `kas/` directory belongs to the upstream checkout, not to this repo.

The build runs on a separate Linux build host with real disk (50-100 GB or more) and Docker, through `kas-container`, not on the machine used for flashing. `build-image.sh` does the following, in order:

1. Validates `config/controller.toml`, `config/terminal.toml` and `config/frontend.toml`, before any cargo build.
2. Clones a fresh upstream checkout (or reuses the one named by `YOCTO_CHECKOUT`).
3. Cross-compiles `controller/` and `daemon/` for `aarch64-unknown-linux-gnu` with `+crt-static`.
4. Copies the two binaries and `controller.toml` / `terminal.toml` into the agent recipes' `files/` directories.
5. Syncs `frontend/` (minus `tests/` and `node_modules/`) into the webapp recipe and generates `config.generated.js` from `config/frontend.toml`.
6. Copies `build/overlay/` over the example layer.
7. Runs `kas-container build --target superbird-kiosk-prod-image kas/example-kiosk.yml` inside the checkout.
8. Stages the resulting flashthing zip as `build/artifact/superbird-kiosk-prod-image-flashthing-YYYYMMDD.zip`.

`build/artifact/` is gitignored apart from its `.gitkeep`; the zips are never committed and are not tracked by Git LFS.

```sh
build/build-image.sh                                    # fresh clone every run by default
YOCTO_CHECKOUT=/path/to/checkout build/build-image.sh   # reuse (or create) a persistent checkout
build/flash-image.sh                                    # auto-detect device, flash newest build/artifact/*.zip
build/flash-image.sh 003:012 build/artifact/superbird-kiosk-prod-image-flashthing-20261121.zip
```

The decision trail (why `kas-container` over bare `bitbake`, why each recipe-level customization exists, build and flash troubleshooting history) is in [`research/yocto-build-decisions.md`](research/yocto-build-decisions.md).

## What the BSP provides and what this project adds

`yocto-superbird` is a from-scratch Yocto build for the same Amlogic hardware: a real systemd init, a BitBake recipe story for adding packages, and a current userspace. The target is `aarch64-unknown-linux-gnu` (`TARGET_SYS = aarch64-poky-linux` in the BitBake banner).

This project changes the upstream kiosk example layer only through `build/overlay/`:

- **Kiosk webapp.** The `superbird-kiosk-default-webapp` recipe is overridden to install this repo's `frontend/` tree (recursively, vendored xterm.js and the generated config included) as the kiosk's default page, so the kiosk boots straight into the project's UI.
- **Agents.** `controller-agent` and `thing-revived-terminal-agent` are each a prebuilt static Rust binary plus a systemd unit, installed by a small recipe with `do_compile` and `do_configure` set to `noexec`, a hand-written `do_install()` and `inherit systemd`. Both services are enabled in the image. `controller.toml` and `terminal.toml` are installed under `/etc/thing-revived/`, and the root filesystem is read-only at runtime.
- **Image contents.** `superbird-kiosk-prod-image.bbappend` adds `bash` (not in the minimal base image), the two agent packages, `thing-revived-ntp-config`, the accounts recipe and `pam-plugin-faillock`. The build needs no SSH public key: none is baked. The OpenSSH client, server and key generator come from the upstream image recipe, not from this overlay.
- **USB host-mode modules.** The same `.bbappend` installs `kernel-module-usbnet`, `-mii`, `-r8152`, `-ax88179-178a`, `-asix`, `-cdc-ether`, `-cdc-ncm`, `-rndis-host`, `-usb-storage`, `-uas` and `-sd-mod`, since the image otherwise ships no `/lib/modules`. These give USB Ethernet and mass storage through a USB-C dock. Ethernet through a dock is verified on hardware; mass storage is not.
- **USB Wi-Fi.** The `thing-revived-wifi.cfg` kernel fragment re-enables WLAN, cfg80211 and mac80211 after the BSP's `superbird-disable.cfg` (extra `*.cfg` fragments are merged after the three named ones, so it wins) and builds the adapter drivers as modules: the D-Link DWA-171 rev A1 (`rtw88_8821au`), B1 (`mt76x0u`) and C1 (`rtw88_8821cu`), the other rtw88 USB chips, `mt76x2u`, `mt7601u`, `rt2800usb` and `rtl8xxxu`. The `.bbappend` installs those modules with the matching `linux-firmware-*` packages, `wpa-supplicant`, `iw` and `wireless-regdb-static`. A `linux-firmware` `.bbappend` carves out a one-file `linux-firmware-mt7610u` package for the B1 instead of the roughly 62 MB mediatek package. Wi-Fi works on real hardware, verified on 2026-11-22 with a DWA-171 rev C1 (`rtw88_8821cu`) behind a USB-C dock in host mode; the other adapters are untested. The C1 first enumerates as a Realtek storage device (`0bda:1a2b`), so the `thing-revived-wifi-udev` recipe installs a udev rule that runs `usb_modeswitch -K`, and a `usb-modeswitch` `.bbappend` drops its tcl dependency. The `thing-revived-wifi-config` recipe installs the runtime pieces: a per-interface `thing-revived-wpa@.service`, a udev rule that starts it when a wireless interface appears, a `systemd-networkd` DHCP file for wireless interfaces and a tmpfiles entry for the root-only config under `/var/lib/thing-revived/`. No credentials are shipped; networks are added from the on-device settings menu, see [`wifi.md`](wifi.md).
- **Bluetooth HID.** The `thing-revived-uhid.cfg` fragment enables `CONFIG_UHID`, which BlueZ's input plugin needs to turn a paired peripheral's HID reports into an evdev input device.
- **Accounts.** The `thing-revived-accounts` recipe seeds `kiosk` locked through extrausers, locks root, mounts a writable overlay on `/etc` (upper layer on `/var`) through `thing-revived-etc.service`, installs the sshd drop-in (`Match User kiosk` allows password or key, everything else key-only), the key store tmpfiles entry and `thing-revived-provision.service`. The `thing_revived_pam` rootfs post-process in the image bbappend (not a libpam bbappend, which would change libpam's signature and force a chromium rebuild) removes `nullok`, sets `rounds=` and adds the `pam_faillock` lines (`deny=5 unlock_time=600 fail_interval=900`), the `sudo` bbappend grants the `sudo` group, and the image bbappend asserts the result at build time. After a build, `python3 build/tools/check_image_accounts.py <built .wic>` checks the account rules on the image. Two development flags, `THING_REVIVED_ENABLE_ADB=1` and `THING_REVIVED_ENABLE_UART=1`, keep the unauthenticated adb root shell and the UART root autologin; both are off by default. Behaviour and limits: [`users.md`](users.md). None of this has been through a real bitbake build yet.
- **USB-C role default.** The image boots with the port in device (gadget) mode. The `superbird-usb-role` `.bbappend` installs a systemd drop-in, `10-thing-revived-default-device.conf`, that runs `superbird-usb-role to-device` unless `/var/lib/superbird-usb-role/boot-role` contains `host`, in which case it runs `to-host --no-revert`. ADB dev builds (`THING_REVIVED_ENABLE_ADB=1`, which ship `/etc/usb-debugging-enabled`) force device mode regardless of a persisted host role, so the dev link always exists; production builds honour a persisted host role. Upstream persists the role in that file, but `/var` is the data partition created at first boot, so the image cannot ship it. Consequence: the USB gadget link (CDC-NCM SSH) is up at boot. Host mode for USB Wi-Fi, Ethernet or storage adapters is opt-in: run `superbird-usb-role persist host` and reboot, it takes effect from the next boot; `superbird-usb-role persist device` returns to the default. Reflashing in burn mode always works.

## Reaching a flashed device

In device (gadget) role the device enumerates over USB as a CDC-NCM network device and accepts SSH as `kiosk` (password or key, see [`users.md`](users.md)); root login is refused. The `kiosk` password is set on the first boot through the M button screen, so a freshly flashed device has no remote login until then. Device role is the boot default, so this works out of the box unless host mode was persisted with `superbird-usb-role persist host` on a production build (ADB dev builds always boot device mode, and `/var` survives reflashing, so a persisted host role stays in effect on production builds until `persist device` or `persist clear`). See [`hardware.md`](hardware.md) for the role table and the networking facts for the dock.

The image's BusyBox is stricter than GNU coreutils: `head -10` fails (use `head -n 10`), and there is no `timeout` applet.

## Time sync

The device has no RTC, so the clock is wrong at every boot until a network time source answers. The image syncs it with `systemd-timesyncd` over NTP, set up by the `thing-revived-ntp-config` recipe: it installs the drop-in `/etc/systemd/timesyncd.conf.d/10-thing-revived-ntp.conf` (`pool.ntp.org` servers, with `time.cloudflare.com`, `time.google.com` and two anycast IP addresses as fallbacks, and a 10 s connection retry) and enables the unit explicitly, since the image's systemd preset is `disable *`. A `superbird-clock` `.bbappend` removes upstream's mask of `systemd-timesyncd`; `superbird-clock` itself stays and still bumps the clock forward to the saved or build-time floor early in boot. `timesyncd` follows `systemd-networkd`, so the clock syncs once any network (Wi-Fi or dock Ethernet) is up. The daemon reports clock status from `timedatectl` (`NTPSynchronized`). Verified on hardware on 2026-11-22: the clock syncs by itself once a network is up.

## Flashing

The flashing tool is [`flashthing-cli`](https://crates.io/crates/flashthing-cli) (`cargo install flashthing-cli`). `build/flash-image.sh` expects it at `~/.cargo/bin/flashthing-cli` unless `FLASHTHING_CLI` points elsewhere. Entering burn mode (hold buttons 1+4 while plugging in USB) and the post-flash boot (unplug, then plug back in with no buttons held) are physical actions only the person at the device can do; burn mode shows up in `lsusb` as `1b8e:c003`. `flashthing-cli` only writes the image and never reboots into it.

The script wraps the tool with device auto-detection and result checking. `flashthing-cli` exits 0 even when it fails and colours its log with ANSI escapes even through a pipe, so the script strips the escape codes and judges success from the output: the final `flashthing_cli: done!` line must be present and no `ERROR flashthing_cli` or `failed to flash device` line may appear. A missing `done!` counts as failure. A transient `USB error: Operation timed out` is retried automatically, up to twice. A wedged USB host controller (`USB error: Entity not found`, or `xhci_hcd: Trying to add endpoint 0x81 without dropping it` in `dmesg`) is not software-recoverable: the script asks you to unplug the device and replug it holding buttons 1+4, waits for `1b8e:c003`, then retries, at most twice. Details and arguments are in [`build/README.md`](../build/README.md).

## Reflashing vs. live-patching

The reproducible path is to rebuild the image from this repo and reflash it. Patching files on a running device is a one-off on a single unit and does not carry over to the next image. The one time a read-write remount was used for a fast edit-test loop is recorded in [`research/yocto-build-decisions.md`](research/yocto-build-decisions.md).
