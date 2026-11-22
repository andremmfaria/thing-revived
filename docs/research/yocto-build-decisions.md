# Yocto Build/Flash: Decision and Troubleshooting History

The decision trail and troubleshooting history behind the `yocto-superbird` build/flash workflow, moved out of `yocto-build-and-flash.md` (which now covers only the current, dateless workflow). See [`yocto-build-and-flash.md`](../yocto-build-and-flash.md) for the current description.

## Why `yocto-superbird` instead of stock firmware

Stock Car Thing firmware is a DeskThing-modified, Buildroot-based userland with a 32-bit `armv7` Chromium 69 kiosk (**Correction 2026-11-22: the owner confirms the device runs Chromium 68; 69 was recorded here in error**) on top of a 64-bit-reporting kernel, real, but aging, under-documented, and missing a usable init system, a real package manager, or any SSH story at all (see [`bash-availability.md`](bash-availability.md) and [`bt-hid-terminal-feasibility.md`](bt-hid-terminal-feasibility.md) for what that firmware can't do). `yocto-superbird` replaces the whole OS with a from-scratch Yocto build targeting the same Amlogic hardware: a real systemd init, a real Yocto/BitBake recipe story for adding packages, and a genuinely current userspace, at the cost of a full reflash and losing whatever Spotify or DeskThing left on the device. This project had already evaluated and shelved `yocto-superbird` once before (see [`architecture-decision.md`](architecture-decision.md)) when stock firmware's measured RAM headroom looked sufficient; the decision to actually commit to it was made deliberately, with sign-off, specifically to get systemd, SSH, and a real build pipeline for custom on-device daemons that stock firmware's Buildroot/`supervisord` setup can't support cleanly.

## Why `kas-container` over bare `bitbake`

`pip install --user kas` is the only package-manager step needed on the build host; it provides both the `kas` CLI and `kas-container`, a thin Docker wrapper around `kas` that runs the whole BitBake build inside a container with the right toolchain and sstate mounts already wired up. This project used `kas-container` exclusively, never bare `kas`, so no local toolchain bootstrap was needed on the build host beyond Docker itself.

## No shortcuts exist — every customization needs a real build

Before committing to a multi-hour build cycle, the obvious shortcut was checked and ruled out: GitHub's Releases API and Actions-artifacts API for `JoeyEamigh/yocto-superbird` were both queried directly and came back empty — zero releases, zero workflow artifacts. The flashthing-zip filenames mentioned in the project's own README are build *outputs* a user produces locally, not something ever published for download. There is no prebuilt image anywhere that could be patched or repackaged instead of building; every customization required an actual `kas-container build` from source.

## The cross-compile target gotcha: this BSP is genuinely 64-bit

This was the single most expensive mistake available here, because it does *not* fail at `cargo build` time — it passes cross-compilation cleanly and only surfaces as a failure deep inside a multi-thousand-task Yocto image build, after everything else has already succeeded.

Stock firmware's userspace is 32-bit `armv7-unknown-linux-gnueabihf` despite reporting an `aarch64` kernel (see [`hardware-verification.md`](hardware-verification.md)). `yocto-superbird`'s userspace is genuinely 64-bit — `aarch64-unknown-linux-gnu` — confirmed directly from the kas/BitBake build configuration log, which prints `TARGET_SYS = aarch64-poky-linux` in its "Build Configuration" banner.

A wrong-arch prebuilt binary (e.g. accidentally shipping an `armv7` build into a recipe meant for this BSP) builds and packages into the image without complaint, and only fails much later at BitBake's `do_package_qa` step, with an error like:

```
ERROR: ... QA Issue: Architecture did not match (ARM, expected AArch64) ...
```

— by which point the rest of the image (kernel, Chromium, Weston, the whole dependency graph) has already built. The fix is simply building for the right target in the first place: `aarch64-unknown-linux-gnu`, with the linker configured in `.cargo/config.toml` (`linker = "aarch64-linux-gnu-gcc"`, installed via `sudo apt-get install -y gcc-aarch64-linux-gnu` if missing).

One more wording trap on this target specifically: building with `RUSTFLAGS="-C target-feature=+crt-static"` on `aarch64-unknown-linux-gnu` produces a binary `file` reports as `ELF 64-bit LSB pie executable, ARM aarch64, ... static-pie linked`, not the plain `statically linked` seen on the `armv7` target. This is the normal, expected static-linking mode for glibc on aarch64 (it keeps PIE/ASLR while still avoiding a runtime glibc-version dependency), not a build problem.

## Customization approach and the `CONFIG_UHID` discovery

Three recipe-level changes were made to get the current image: making the project's own webapp the default kiosk page (overriding the `superbird-kiosk-default-webapp` recipe's `SRC_URI`), installing each prebuilt Rust daemon as its own systemd-managed package (a reusable BitBake recipe template: `S = "${UNPACKDIR}"`, `INSANE_SKIP:${PN} += "already-stripped"`, `do_compile[noexec]`/`do_configure[noexec]`, a hand-written `do_install()`, `inherit systemd`), and adding missing base packages (`bash`, `openssh-ssh`, `openssh-keygen`) plus the two new daemons to `superbird-kiosk-prod-image.bb`'s `IMAGE_INSTALL:append`.

The worked example of a kernel-config-fragment change: BlueZ's `bluetoothd` input plugin creates a virtual HID input device from a paired peripheral's HID reports via `/dev/uhid` — but on the kernel actually running, `CONFIG_UHID` was unset, `/dev/uhid` didn't exist, and no `uhid.ko` existed anywhere under `/lib/modules`. BT keyboard pairing completed cleanly (confirmed via `bluetoothctl`) but produced zero real input — confirmed live, on-device, before touching the kernel config, not assumed from reading Kconfig help text. The fix lives in `meta-superbird/recipes-kernel/linux/linux-superbird/superbird.cfg`, a kernel config *fragment* BitBake merges on top of the defconfig, not a full defconfig edit:

```diff
--- a/meta-superbird/recipes-kernel/linux/linux-superbird/superbird.cfg
+++ b/meta-superbird/recipes-kernel/linux/linux-superbird/superbird.cfg
@@ -113,6 +113,15 @@ CONFIG_SERIAL_DEV_BUS=y
 CONFIG_SERIAL_DEV_CTRL_TTYPORT=y
 CONFIG_RFKILL=y

+# uhid: BlueZ bluetoothd input plugin creates a virtual HID input device
+# from a paired peripherals HID reports via /dev/uhid. Without this, BT
+# keyboard/mouse/gamepad pairing completes but produces zero real input -
+# confirmed live on-device. This is the only kernel-side piece bluetoothd
+# needs for HID-over-GATT/HID profile input to actually reach evdev.
+CONFIG_UHID=y
+
 # GPIO character device - userspace bring-up of GPIOX_17 (BT chip enable)
 # uses libgpiod which talks to /dev/gpiochipN. CONFIG_GPIO_SYSFS is the
 # deprecated alternative; chardev is the supported path.
```

This is a base-BSP-layer change (`meta-superbird/`), unlike the distro/image-layer customizations above — kernel config belongs with the kernel recipe. It needs a full `kas-container build` to take effect; there is no "just rebuild the kernel module" shortcut for a Kconfig option baked into the main kernel image, and no live-patch path either.

## Build mechanics observed

`kas/example-kiosk.yml`'s own `target:` list names six images; `--target superbird-kiosk-prod-image` scopes the build to just the one actually needed for flashing, avoiding the real build-time cost of the four OTA delta targets nobody flashes. `SSTATE_MIRRORS` (configured in `kas/base.yml`) matters a lot in practice: even a build that looks like a cold rebuild after a recipe change finishes in minutes, not hours, because the overwhelming majority of BitBake's task graph is an sstate-cache hit every time — only the first build (before the arch-mismatch fix above) actually failed a task; every build after succeeded outright, redoing only a few dozen to a couple hundred tasks out of 6000+ attempted.

A background launcher shaped like `nohup ssh devel "... &" & echo pid $!` reports completion almost immediately — that's the local wrapper shell exiting right after it forks the remote command, not the real `docker run`/BitBake process actually finishing; treat that notification as meaningless for build-progress purposes. The reliable pattern is finding the actual `docker run` PID on the build host and tying a monitor to that specific PID (`ssh devel "ps aux | grep 'docker run'"`, then poll `kill -0 <pid>`).

## Flashing failure modes observed

The real flashing tool is [`flashthing-cli`](https://crates.io/crates/flashthing-cli) — not the older Python `superbird-tool`, which this project doesn't use (no Python anywhere in this repo). First run needs `sudo <full-path-to>/flashthing-cli --setup` using the absolute path, since `sudo` doesn't inherit `~/.cargo/bin` on `PATH`.

A mid-flash `USB error: Operation timed out` happened once and is a real but recoverable failure mode, not corruption: the chip drops back to its bare BootROM state and waits for a fresh connection; the fix is simply rerunning `flashthing-cli <zip>` again, no need to re-enter burn-mode by hand. `flashthing-cli` only writes the image; it never reboots into it — the device sits in mask-ROM mode until the human does the physical unplug/replug. What actually gets written, as two separate `dd`-equivalent streams: a large sparse main user-area write (~1.43GB on past builds) and a smaller second segment (~200MB) — roughly 1.5-1.6GB of real bytes landed on the eMMC, versus the flashthing zip's own ~345-363MB sparse/compressed size. Don't conflate the two numbers.

## Live-patching vs. reflashing: what actually happened once

At one point, the running device's root filesystem was remounted read-write for a fast edit-test loop; those changes landed as real writes to the actual eMMC partition and survived without a reflash — a deliberate dev-convenience workaround, not the intended production mode, since the kernel command line mounts root `ro` by default on every normal boot.

This bit the project directly once: a `CONFIG_UHID` reflash wiped out an entire session's worth of live-patched frontend and `controller-agent` fixes that had accumulated since the last recipe sync. Everything after that last-synced commit — a button-role swap, the settings-menu UI, confirm/long-press handling, the paired-devices screen, the persistent passkey-entry screen, the HID auto-confirm pairing fix — existed only as live patches on the running device, never synced into the Yocto recipe sources on the build host. The rebuild necessarily used the recipes as they stood, so the fresh image silently reverted to that older snapshot. The fix was simple only because every change was still safely committed in this git repo (never lost, just not yet reflected in the booted image) — cross-compile the current agents from the repo's current `main`, redeploy both onto the freshly-reflashed device the same way, and verify hashes match the repo before considering it done.

**The operational rule this established:** before planning any kernel/recipe-level rebuild+reflash, check first whether the BSP checkout's recipe sync is current with the application repo's `main` branch. If it's behind (as it almost always will be, since live-patching is the fast iteration path used for everything that doesn't strictly require a kernel change), redeploying the current `main` state onto the freshly-flashed device is a required step of the reflash procedure, not an optional afterthought.

There's also a quieter risk worth flagging: this BSP uses an A/B boot-slot scheme (`root=PARTLABEL=root_a` on the kernel cmdline implies a `root_b` fallback partition exists). A live-patched state sitting on the currently active slot could simply disappear with no error and no warning if the active slot ever flips to the other one.
