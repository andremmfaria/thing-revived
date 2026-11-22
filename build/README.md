# build/

Orchestration for producing and flashing a yocto-superbird kiosk image for this device. This directory does not vendor or commit any Yocto source/layer tree: the upstream [`yocto-superbird`](https://github.com/JoeyEamigh/yocto-superbird) repo is cloned fresh (or reused) at build time by `build-image.sh`, and only this project's own small overlay files get copied into that checkout before the real build runs. See `docs/yocto-build-and-flash.md` for the full narrative and the manual, step-by-step equivalent of everything these scripts automate.

## Layout

- `overlay/` - small text files only (recipes, `.service` units, udev rules, kernel config fragments, `.bbappend` files), laid out to mirror their exact destination paths inside the upstream checkout's existing `examples/meta-superbird-kiosk-example/` layer. No binaries and no full frontend copy live here; `build-image.sh` generates and places those at build time.
- `tools/` - `toml_to_js.py` (TOML to `window.THING_CONFIG` script generator, stdlib only, Python 3.11+) and its unittest suite `test_toml_to_js.py`.
- `build-image.sh` - clones/reuses the upstream checkout, cross-compiles this repo's Rust agents, syncs `frontend/` and `overlay/` into the checkout, runs the real `kas-container build`, and stages the resulting flashthing zip into `artifact/`.
- `flash-image.sh` - flashes a built artifact zip to a physical device over USB in Amlogic burn-mode.
- `artifact/` - build outputs (flashthing zips) written by `build-image.sh` and read by `flash-image.sh`. The directory contents are gitignored; only `.gitkeep` is tracked.

## Usage

### `build-image.sh`

```sh
build/build-image.sh
```

Environment variables:

- `THING_REVIVED_SSH_PUBKEY_FILE`, `THING_REVIVED_ENABLE_ADB`, `THING_REVIVED_ENABLE_UART`, `KEY_ONLY`: see the next section.
- `YOCTO_CHECKOUT` (optional), path to an existing yocto-superbird checkout to reuse (its sstate/downloads carry over between runs). If unset, a fresh `git clone --depth 1` is made into a unique `/tmp/yocto-superbird-build-$$` directory every run. If set to a path that doesn't exist yet, the clone is made there instead, so you can point it at a persistent location on a build host with real disk.

What it does, in order: validate `config/*.toml` and the SSH public key (both fail before any cargo build) → clone/reuse the checkout → cross-compile `controller/` and `daemon/` for `aarch64-unknown-linux-gnu` with `+crt-static` → copy the two binaries and `controller.toml` / `terminal.toml` into the checkout's `examples/meta-superbird-kiosk-example/recipes-extended/{controller-agent,thing-revived-terminal-agent}/files/` → sync this repo's `frontend/` (minus `tests/` and `node_modules/`) into the webapp recipe's `files/webapp/` and generate `config.generated.js` there from `config/frontend.toml` → copy `overlay/` on top of the checkout's example layer → write the baked SSH key and `kas/thing-revived-flags.yml` into the checkout → run `kas-container build --target superbird-kiosk-prod-image kas/example-kiosk.yml:kas/thing-revived-flags.yml` from inside the checkout → copy the resulting flashthing zip into `build/artifact/superbird-kiosk-prod-image-flashthing-YYYYMMDD.zip`. Staging resolves symlinks in the deploy dir, ignores `-env-only`, dangling and under-1 MB candidates, picks the newest real zip, and checks the copy's size and zip integrity before moving it into place, so a failed staging never leaves a bogus zip in `artifact/`. `STAGE_ONLY=1 [DEPLOY_DIR=...] build/build-image.sh` runs just this staging step.

### SSH key and dev flags

A real build bakes the build host's SSH public key into the image for the `kiosk` user (the image is key-only for SSH and root is locked, see `docs/research/user-management-plan.md`). The key is read at build time from `THING_REVIVED_SSH_PUBKEY_FILE`, default `~/.ssh/id_ed25519.pub` on the build host. It is never committed: `build-image.sh` writes it only into the scratch checkout (`recipes-extended/thing-revived-accounts/files/kiosk.authorized_keys`), removes any stale copy first and removes it again on exit, and `.gitignore` has a defensive entry for `*.authorized_keys`. Only the key type, fingerprint and file path are logged, not the comment.

The build fails with a one-line `[build-image] ERROR:` message, before the clone and cargo steps, when the file is missing, unreadable or empty, looks like a private key (`-----BEGIN` or `PRIVATE KEY`), holds no key or more than one key (blank and `#` comment lines are ignored), starts with `authorized_keys` options such as `command=` instead of a key type, or is not accepted by `ssh-keygen -l -f`.

- `KEY_ONLY=1 build/build-image.sh` runs only this key check and exits (0 for a valid key), with no config validation, clone, cargo or kas. `CONFIG_ONLY=1` skips the key check so CI can validate config without a key.
- `THING_REVIVED_ENABLE_ADB=1` keeps `adbd` usable (the `usb-debugging-enabled` flag file stays in the image). adbd then gives an unauthenticated root shell over USB, so use it for development builds only.
- `THING_REVIVED_ENABLE_UART=1` keeps the UART serial console auto-login as root. Development builds only.
- Both flags are independent and off by default, and only the exact value `1` enables one; anything else (including `true` or `yes`) is `0`. A loud warning is logged for each flag that is on.

The flags reach bitbake through an extra kas config the script generates in the scratch checkout, `kas/thing-revived-flags.yml`, and the build runs `kas-container build --target superbird-kiosk-prod-image kas/example-kiosk.yml:kas/thing-revived-flags.yml`. It has a top-level `env:` map naming both variables, so kas adds them to `BB_ENV_PASSTHROUGH_ADDITIONS`, and the normalised values are exported before `kas-container` runs. Whether `kas-container` forwards host variables into its container is not verified, so the same file also sets both variables as literals in `local_conf_header`, which makes the recipes see the value either way (they read `THING_REVIVED_ENABLE_ADB ?= "0"` and `THING_REVIVED_ENABLE_UART ?= "0"`). To confirm on a build host: `kas-container shell kas/example-kiosk.yml:kas/thing-revived-flags.yml -c 'bitbake -e superbird-kiosk-prod-image | grep ^THING_REVIVED_ENABLE'`.

Tests: `build/test-build-image.sh` runs `KEY_ONLY=1` against temp key files (valid, commented, missing, empty, private key, two keys, key with options, garbage, comment only), checks the default path and that `CONFIG_ONLY=1` needs no key, and checks the generated kas content for each flag state. No clone, cargo or kas is run.

### Config, generator and `CONFIG_ONLY` mode

Runtime config lives in the repo-root `config/` directory (one TOML per component; see `config/README.md`). The build validates all three files up front, installs `controller.toml` and `terminal.toml` into the agent recipes (on the device: `/etc/thing-revived/controller.toml` and `/etc/thing-revived/terminal.toml`, marked as CONFFILES, read-only at runtime), and turns `frontend.toml` into `config.generated.js` with `build/tools/toml_to_js.py` because the kiosk browser cannot read TOML. The generator output is deterministic and the committed `frontend/config.generated.js` stub is never modified. The `superbird-kiosk-default-webapp` recipe installs the whole `files/webapp` tree recursively, so new frontend subdirectories need no recipe change.

`CONFIG_ONLY=1 build/build-image.sh` runs only the validate and generate steps, writing `config.generated.js` into `CONFIG_OUT_DIR` (default: a fresh temp dir whose path is printed) with no clone, cargo or kas. `CONFIG_DIR` overrides the config source directory (useful for testing against copies). Tests: `python3 build/tools/test_toml_to_js.py`.

Needs real disk (50-100GB+) and Docker for the `kas-container` step; don't run it on a disk-constrained host (see `docs/yocto-build-and-flash.md`). Other host requirements: cargo with the `aarch64-unknown-linux-gnu` target and `gcc-aarch64-linux-gnu`, git, rsync, Python 3.11+, and `kas-container` (found on `PATH` or at `~/.local/bin/kas-container`).

The kernel also gets a `thing-revived-uhid.cfg` fragment (`CONFIG_UHID=y`) so BlueZ can turn paired Bluetooth HID devices into real input devices.

The image also includes the host-mode USB Ethernet and mass-storage kernel modules (usbnet, mii, r8152, ax88179_178a, asix, cdc_ether, cdc_ncm, rndis_host, usb-storage, uas, sd_mod), installed via the `.bbappend`, so a USB-C dock works after `superbird-usb-role to-host`.

The image also includes USB Wi-Fi support for the D-Link DWA-171 rev A1 (RTL8811AU, rtw88_8821au), rev B1 (MT7610U, mt76x0u) and rev C1 (RTL8821CU, rtw88_8821cu), plus a catch-all of other rtw88 USB chips (8812AU, 8814AU, 8822BU, 8822CU, 8723DU), mt76x2u, mt7601u, rt2800usb and rtl8xxxu, with their firmware, `wpa-supplicant`, `iw` and the static regulatory database. The kernel side is the `thing-revived-wifi.cfg` fragment, which re-enables WLAN, cfg80211 and mac80211 after the BSP's disable fragment; the C1 first shows up as a Realtek storage device (0bda:1a2b) and a udev rule (`thing-revived-wifi-udev`) ejects it so it re-enumerates as the Wi-Fi device (2001:331d). Networks are added from the on-device settings menu (no config or credentials are shipped; design in `docs/wifi.md`) and Wi-Fi needs the USB-C port in host mode, which is the boot default (below), with the adapter on a dock or OTG adapter. Wi-Fi is verified on real hardware (2026-11-22): a D-Link DWA-171 (rtw88_8821cu) through a USB-C dock in host mode, with dock Ethernet also working. The other listed chips are built and packaged but were not individually tested.

The `thing-revived-wifi-config` recipe (`overlay/recipes-extended/thing-revived-wifi-config/`) is what makes Wi-Fi usable at runtime. It installs `thing-revived-wpa@.service` (wpa_supplicant for one interface, config at `/var/lib/thing-revived/wpa_supplicant.conf`, bound to the interface's device unit so unplugging the adapter stops it), `90-thing-revived-wifi-wpa.rules` (udev: start that unit when a `DEVTYPE=wlan` interface is added), `80-thing-revived-wifi.network` (systemd-networkd DHCP for `Type=wlan`, route metric 20 so Ethernet at 10 wins), and `thing-revived-wifi.conf` (tmpfiles.d: `/var/lib/thing-revived` as 0700 and an initial 0600 `wpa_supplicant.conf` with `ctrl_interface` and `update_config=1`, created only if missing). It is added to `IMAGE_INSTALL` in `superbird-kiosk-prod-image.bbappend`. The Wi-Fi frontend logic (`frontend/src/wifi/wifi-logic.js`) needs no recipe entry, because the `superbird-kiosk-default-webapp` recipe installs the whole synced frontend tree.

The image boots with the USB-C port in host mode by default (the `superbird-usb-role` `.bbappend` installs a systemd drop-in, `10-thing-revived-default-host.conf`). Upstream persists the boot role as `/var/lib/superbird-usb-role/boot-role`, but `/var` is the `data` partition carved at first boot, so the image cannot ship that file; the drop-in instead runs `superbird-usb-role to-host --no-revert` unless `boot-role` contains `device`, so no 300 s auto-revert timer is scheduled at boot and `persist device` still works as an override. Consequence: the USB gadget link (CDC-NCM SSH and adb over USB) is down at boot, so access is through the on-device terminal with a Bluetooth keyboard, or over a dock's Ethernet. Recovery is `superbird-usb-role persist device` then reboot, or `superbird-usb-role to-device` for the current boot, from that terminal, or reflashing via burn mode. adbd may still start and try to bind the gadget UDC; whether that is harmless in host role is untested.

### `flash-image.sh`

```sh
build/flash-image.sh [any | bus:device] [path/to/artifact.zip]
```

Both arguments are optional. The first selects the USB device: omit it or pass `any` to auto-detect the single device matching VID:PID `1b8e:c003` (Amlogic burn-mode; the script errors if zero or more than one match), or pass a `bus:device` pair as shown by `lsusb`, either zero-padded (`001:046`) or not (`1:46`), to disambiguate when several are present. The second is a specific zip path; the default is the newest zip in `artifact/`.

Entering burn-mode (hold buttons 1+4 while plugging in USB) and the post-flash boot (unplug, replug with no buttons held) are physical actions the script never performs, it only detects state and reports what's needed.

`flashthing-cli` exits 0 even when it fails, so the script judges the result from its output: success needs the final `flashthing_cli: done!` line and no `ERROR flashthing_cli` or `failed to flash device` line (a non-zero exit status also counts as failure). Anything else ends with `FAILURE` and a non-zero exit status. A transient mid-write `USB error: Operation timed out` (the chip drops to bare BootROM and accepts a fresh connection immediately) is retried automatically, up to twice. A wedged USB host controller (`USB error: Entity not found`, or `xhci_hcd: Trying to add endpoint 0x81 without dropping it` in `dmesg`) is not software-recoverable: the script asks you to unplug the device and replug it holding buttons 1+4, waits for `1b8e:c003` to reappear, then retries (at most twice before giving up).
