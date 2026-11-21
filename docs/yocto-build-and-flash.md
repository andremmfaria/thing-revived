# Building, Customizing, and Flashing yocto-superbird

The device runs [`yocto-superbird`](https://github.com/JoeyEamigh/yocto-superbird), a community-maintained Yocto BSP replacement for the Spotify Car Thing. See [`dependency-surface.md`](dependency-surface.md) for the dependency picture this gives the project, and [`hardware.md`](hardware.md) for the resulting device facts.

## The build/flash scripts

`build/build-image.sh` and `build/flash-image.sh` automate the whole workflow; see `build/README.md` for exact usage. They don't vendor or commit any Yocto source/layer tree into this repo — `build/overlay/` holds only this project's own small recipe/service/config/kernel-fragment text files, laid out to mirror their destination paths inside upstream's `examples/meta-superbird-kiosk-example/` layer (the layer upstream's own `kas/example-kiosk.yml` already includes; no new layer is created). At build time, `build-image.sh` clones (or reuses) a fresh upstream checkout, cross-compiles this repo's `controller/` and `daemon/` agents, copies the binaries and a fresh `frontend/` sync plus `build/overlay/` into that checkout, and runs a real `kas-container build` from inside it:

```sh
build/build-image.sh                 # fresh clone every run by default
YOCTO_CHECKOUT=/path/to/checkout build/build-image.sh   # reuse an existing checkout
build/flash-image.sh                 # auto-detect device, flash newest build/artifact/*.zip
build/flash-image.sh 003:012 build/artifact/superbird-kiosk-prod-image-flashthing-20261121.zip
```

See `build/README.md` for the full flag/argument reference. Full decision trail and manual-debugging walkthrough (why `kas-container` over bare `bitbake`, the exact recipe-level customizations and why each exists, troubleshooting history for build and flash failures): [`research/yocto-build-decisions.md`](research/yocto-build-decisions.md).

## What the BSP provides and what this project adds

`yocto-superbird` targets the same Amlogic hardware with a from-scratch Yocto build: a real systemd init, a real Yocto/BitBake recipe story for adding packages, and a current userspace — `aarch64-unknown-linux-gnu`, confirmed from the kas/BitBake build configuration's `TARGET_SYS = aarch64-poky-linux` banner line.

Layers/recipes this project uses, via `build/overlay/` rather than hand-edited upstream files:

- `examples/meta-superbird-kiosk-example/`: the Chromium-kiosk example layer this project builds on — `conf/distro/superbird-kiosk.conf` (distro-level config), `recipes-extended/` (this project's own daemon recipes plus base packages added to the image), `recipes-core/images/` (the `superbird-kiosk-prod-image.bb` image recipe this project targets).
- The kiosk's default webapp recipe (`superbird-kiosk-default-webapp`) is overridden to point at this project's own `frontend/` instead of upstream's placeholder, so the kiosk boots straight into the terminal UI with no distro/image-recipe changes needed elsewhere. It also ships a vendored xterm.js UMD build and CSS under the same recipe's `do_install()`.
- `controller-agent` and `thing-revived-terminal-agent` are each installed as a prebuilt Rust binary plus a `.service` unit via a small, reusable BitBake recipe (`S = "${UNPACKDIR}"`, `INSANE_SKIP:${PN} += "already-stripped"`, `do_compile[noexec]`/`do_configure[noexec]`, a hand-written `do_install()`, `inherit systemd`).
- `superbird-kiosk-prod-image.bb`'s `IMAGE_INSTALL:append` adds `bash`, `openssh-ssh`, `openssh-keygen` (none of which ship by default on the minimal BusyBox-based kiosk image) plus the two daemon packages above.
- `meta-superbird/recipes-kernel/linux/linux-superbird/superbird.cfg` carries a kernel config fragment enabling `CONFIG_UHID`, needed for BlueZ's `bluetoothd` input plugin to turn a paired BT peripheral's HID reports into a real evdev input device.

## Talking to a flashed device

After a flash and a normal reboot (unplug, then plug back in with no buttons held), the device enumerates over USB as a CDC-NCM network device, not `adb` (which doesn't exist on this BSP). Bring the link up from the host:

```sh
sudo ip link set <enxIFACE> up
sudo dhcpcd -4 --timeout 10 <enxIFACE>
```

`dhcpcd` specifically, not `udhcpc`/`dhclient`/`nmcli`. The device runs the DHCP **server** role on this point-to-point link: the address `dhcpcd` reports back as the lease server is the device's own IP, not the host's. There is no default route on this link — it's a directly-connected `/29` scope-link subnet, which is why `daemon/`'s own host-discovery logic (see [`research/button-knob-clock-design.md`](research/button-knob-clock-design.md)) falls back to probing the subnet rather than parsing a default route.

SSH straight to that address as `root`. The kiosk image ships this wide open: zero authentication, not even a password — worth locking down (e.g. `PermitRootLogin prohibit-password` plus a real key) before this device is anything but a desk toy.

`/root` lives on the read-only ext4 rootfs, which breaks both sides of a normal SSH key workflow. Redirect explicitly to the writable `/var` mount whenever a key or `known_hosts` file is needed on-device:

```sh
mkdir -p /var/lib/thing-revived/.ssh
ssh-keygen -t ed25519 -N '' -f /var/lib/thing-revived/.ssh/id_ed25519 -q
ssh -i /var/lib/thing-revived/.ssh/id_ed25519 \
    -o UserKnownHostsFile=/var/lib/thing-revived/.ssh/known_hosts \
    -o StrictHostKeyChecking=accept-new ...
```

This BSP's BusyBox is stricter than GNU coreutils about flag syntax: no GNU long-option-style short flags (`head -10` fails; use `head -n 10`), and no `timeout` applet at all — bound a command another way (e.g. background it and `sleep` + `pkill -f <exact path>`) if one is needed.

## Flashing

The flashing tool is [`flashthing-cli`](https://crates.io/crates/flashthing-cli) (`cargo install flashthing-cli`), not the older Python `superbird-tool` (no Python anywhere in this repo). Entering burn-mode (hold buttons 1+4 while plugging in USB) and the post-flash boot (unplug, then plug back in normally with no buttons held) are both physical actions only the human at the device can do; confirm burn-mode via `lsusb | grep 1b8e:c003`. `flashthing-cli` only writes the image; it never reboots into it. `build/flash-image.sh` wraps this tool with device auto-detection and transient-USB-timeout retry handling — see `build/README.md`.

## Live-patching vs. reflashing

A running device's root filesystem can be remounted read-write (`mount -o remount,rw /`) for a fast edit-test loop; changes pushed via `scp` plus a `systemctl restart` land as real writes to the active eMMC partition and survive without a reflash. This is a dev-convenience workaround, not the intended production mode: the kernel command line mounts root `ro` by default on every normal boot, and this BSP's A/B boot-slot scheme means a live-patched state on the active slot can disappear with no warning if the active slot ever flips. A reflash from the real, recipe-built image is the only way to get a result that's reproducible and version-controlled rather than a one-off hand-patched snapshot that exists only on one physical unit. Before any kernel/recipe-level rebuild+reflash, confirm the BSP checkout's recipe sync is current with this repo's `main` branch — redeploying current `main` onto a freshly-flashed device is a required step, not optional, since live-patching is the usual fast-iteration path and the checkout can otherwise silently regress on rebuild.
