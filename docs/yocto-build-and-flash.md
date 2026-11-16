# Building, Customizing, and Flashing yocto-superbird

This is the full account of the night this project moved off stock firmware and onto [`yocto-superbird`](https://github.com/JoeyEamigh/yocto-superbird), a community-maintained Yocto BSP replacement for the Spotify Car Thing. It covers why this OS was chosen, how the build environment was set up, the exact recipe-level changes made to customize the image, how the build was run and monitored, how to talk to a device freshly flashed with it, the flashing procedure itself, and the operational distinction between live-patching a running device and doing a real reflash. See [`dependency-surface.md`](dependency-surface.md) for how this supersedes that document's earlier "yocto-superbird: never used" finding, and [`hardware.md`](hardware.md) for how the device's hardware facts differ once it's running this BSP instead of stock firmware.

## 1. Why yocto-superbird instead of stock firmware

Stock Car Thing firmware (the one every other doc in this repo up to this point describes) is a DeskThing-modified, Buildroot-based userland with a 32-bit `armv7` Chromium 69 kiosk on top of a 64-bit-reporting kernel — real, but aging, under-documented, and missing things like a usable init system, a real package manager, or any SSH story at all (see [`research/bash-availability.md`](research/bash-availability.md) and [`research/bt-hid-terminal-feasibility.md`](research/bt-hid-terminal-feasibility.md) for what that firmware can't do). `yocto-superbird` replaces the whole OS with a from-scratch Yocto build targeting the same Amlogic hardware: a real systemd init, a real Yocto/BitBake recipe story for adding packages, and a genuinely current userspace, at the cost of a full reflash and losing whatever Spotify or DeskThing left on the device. This project had already evaluated and shelved `yocto-superbird` once before (see [`research/architecture-decision.md`](research/architecture-decision.md)) when stock firmware's measured RAM headroom looked sufficient; the decision to actually commit to it this time was made deliberately, with sign-off, specifically to get systemd, SSH, and a real build pipeline for custom on-device daemons that stock firmware's Buildroot/`supervisord` setup can't support cleanly.

## 2. Cloning and setting up the build environment

The Yocto build was done on **devel**, a separate 56-core, 1.4TB-free build host — not on this project's normal home, vermelho, whose ~33GB of free disk is nowhere near enough. A Yocto build of this size genuinely needs 50-100GB+ of scratch space for `tmp/`, sstate cache, and downloads; starting it on a small host just fails mid-build once disk fills up, it doesn't fail fast at the start.

Setup on devel:

```sh
git clone --depth 1 https://github.com/JoeyEamigh/yocto-superbird
pip install --user kas
```

`pip install --user kas` is the only package-manager step needed; it provides both the `kas` CLI and `kas-container`, a thin Docker wrapper around `kas` that runs the whole BitBake build inside a container with the right toolchain and sstate mounts already wired up — this project used `kas-container` exclusively, never bare `kas`, so no local toolchain bootstrap was needed on devel at all beyond Docker itself.

Repo layout, as relevant to this project's customizations:

- `kas/` — kas config fragments (`base.yml`, `example-kiosk.yml`, etc.) that compose into a full build configuration via `includes:`.
- `examples/meta-superbird-kiosk-example/` — a complete example Yocto layer implementing a Chromium-kiosk image: `conf/distro/superbird-kiosk.conf` (the distro-level config knobs), `recipes-extended/` (daemons and packages added on top of the base image, including this project's new recipes, see §5), and `recipes-core/images/` (the actual `.bb` image recipes, e.g. `superbird-kiosk-prod-image.bb`).
- `meta-superbird/` — the base BSP layer: kernel, bootloader, device tree, core distro policy. Not touched by this project's customizations; every change made lived in the example kiosk layer instead.

## 3. No shortcuts exist — every customization needs a real build

Before committing to a multi-hour build cycle, the obvious shortcut was checked and ruled out: GitHub's Releases API and Actions-artifacts API for `JoeyEamigh/yocto-superbird` were both queried directly and came back empty — zero releases, zero workflow artifacts. The flashthing-zip filenames mentioned in the project's own README are build *outputs* a user produces locally, not something ever published for download. There is no prebuilt image anywhere that could be patched or repackaged instead of building; every one of this project's three customizations (§5) required an actual `kas-container build` from source.

## 4. The cross-compile target gotcha: this BSP is genuinely 64-bit

This is the single most expensive mistake available here, because it does *not* fail at `cargo build` time — it passes cross-compilation cleanly and only surfaces as a failure deep inside a multi-thousand-task Yocto image build, after everything else has already succeeded.

Stock firmware's userspace is 32-bit `armv7-unknown-linux-gnueabihf` despite reporting an `aarch64` kernel (see [`hardware.md`](hardware.md)). `yocto-superbird`'s userspace is genuinely 64-bit — `aarch64-unknown-linux-gnu` — confirmed directly from the kas/BitBake build configuration log, which prints `TARGET_SYS = aarch64-poky-linux` in its "Build Configuration" banner. Don't assume the two firmwares share a target just because they're the same physical device; always check that line before cross-compiling anything new for this BSP.

A wrong-arch prebuilt binary (e.g. accidentally shipping an `armv7` build into a recipe meant for this BSP) builds and packages into the image without complaint, and only fails much later at BitBake's `do_package_qa` step, with an error like:

```
ERROR: ... QA Issue: Architecture did not match (ARM, expected AArch64) ...
```

— by which point the rest of the image (kernel, Chromium, Weston, the whole dependency graph) has already built. The fix is simply building for the right target in the first place: `aarch64-unknown-linux-gnu`, with the linker configured in `.cargo/config.toml` (`linker = "aarch64-linux-gnu-gcc"`, installed via `sudo apt-get install -y gcc-aarch64-linux-gnu` if missing).

One more wording trap on this target specifically: building with `RUSTFLAGS="-C target-feature=+crt-static"` on `aarch64-unknown-linux-gnu` produces a binary `file` reports as `ELF 64-bit LSB pie executable, ARM aarch64, ... static-pie linked`, not the plain `statically linked` seen on the `armv7` target. This is the normal, expected static-linking mode for glibc on aarch64 (it keeps PIE/ASLR while still avoiding a runtime glibc-version dependency) — it is not a build problem, just a different and correct word for the same thing.

## 5. Customizing the image

Three concrete recipe-level changes were made tonight, all inside `examples/meta-superbird-kiosk-example/`, none inside the base `meta-superbird` layer.

### a. Making a custom webapp the default kiosk page

`CHROMIUM_KIOSK_URL` in `examples/meta-superbird-kiosk-example/conf/distro/superbird-kiosk.conf` already defaults to `file:///usr/share/superbird-kiosk-default/index.html`. That means making a different webapp the default kiosk page needs **no distro-knob edit at all** — the entire mechanism is overriding what the `superbird-kiosk-default-webapp` recipe actually installs to that path (its `SRC_URI` and `do_install()`). Point that recipe's `SRC_URI` at this project's own webapp files instead of upstream's placeholder, and the kiosk boots straight into it with zero changes anywhere else in the distro or image recipe.

This recipe later grew a `vendor/` subdirectory specifically to ship a vendored xterm.js UMD build and its CSS alongside the webapp's own files, installed by the same recipe, into the same `do_install()` — not a separate package, since it's purely static assets consumed by the one webapp.

### b. Installing a prebuilt Rust daemon as its own systemd-managed package

Two separate daemons were added this way tonight: `thing-revived-terminal-agent` (first) and, later in the same session, `control-surface-agent` (the buttons/knob bridge daemon) — the exact same recipe pattern, reused verbatim for the second package. This is a reusable template worth documenting in general, since this project will need it again for any future prebuilt Rust binary shipped to this BSP:

```bitbake
SUMMARY = "..."
LICENSE = "..."

SRC_URI = "file://<binary-name> file://<unit-name>.service"

S = "${UNPACKDIR}"

INSANE_SKIP:${PN} += "already-stripped"
do_compile[noexec] = "1"
do_configure[noexec] = "1"

do_install() {
    install -d ${D}${bindir}
    install -m 0755 ${UNPACKDIR}/<binary-name> ${D}${bindir}/<binary-name>
    install -d ${D}${systemd_system_unitdir}
    install -m 0644 ${UNPACKDIR}/<unit-name>.service ${D}${systemd_system_unitdir}/
}

inherit systemd
SYSTEMD_SERVICE:${PN} = "<unit-name>.service"
SYSTEMD_AUTO_ENABLE:${PN} = "enable"

FILES:${PN} += "${systemd_system_unitdir}/<unit-name>.service"
```

The key pieces, and why each is there:

- `S = "${UNPACKDIR}"` — tells BitBake the "source" is just the unpacked `SRC_URI` files as-is, no build tree to configure.
- `INSANE_SKIP:${PN} += "already-stripped"` — the binary arrives pre-stripped (built on the host with `cargo build --release`), and without this the QA step flags it as suspicious.
- `do_compile[noexec] = "1"` / `do_configure[noexec] = "1"` — there is nothing to compile or configure; this is the manual equivalent of what `bin_package.bbclass` gives you automatically, done explicitly instead of inheriting that class, for more control over `do_install()`.
- A hand-written `do_install()` that copies the binary and `.service` unit into place — this is the only "real" step in the recipe.
- `inherit systemd` + `SYSTEMD_SERVICE:${PN}` + `SYSTEMD_AUTO_ENABLE:${PN}` — this BSP's init is systemd (not `supervisord` like stock firmware, see [`getting-started.md`](getting-started.md) for that older pattern); this is the systemd-equivalent of adding a `supervisor.d/*.conf` file, and makes the unit start automatically on every boot with no manual `systemctl enable` step needed on-device.

### c. Adding missing base packages and the two new daemons to the image

The kiosk image is minimal BusyBox-based and ships neither `bash`, `openssh-ssh`, nor `openssh-keygen` by default. `superbird-kiosk-prod-image.bb`'s `IMAGE_INSTALL:append` was extended to add all three, plus the two new packages from §5b:

```bitbake
IMAGE_INSTALL:append = " bash openssh-ssh openssh-keygen thing-revived-terminal-agent control-surface-agent"
```

## 6. Building

```sh
kas-container build --target superbird-kiosk-prod-image kas/example-kiosk.yml
```

`kas/example-kiosk.yml`'s own `target:` list actually names six images — `superbird-kiosk-dev-image`, `superbird-kiosk-prod-image`, `superbird-kiosk-update-dev`, `superbird-kiosk-update-dev-delta`, `superbird-kiosk-update-prod`, and `superbird-kiosk-update-prod-delta` — a dev build, the prod build actually wanted for flashing, and four OTA delta packages. Running `kas-container build` with no `--target` override would build all six; `--target superbird-kiosk-prod-image` scopes it to just the one image this project actually needs, which matters because the OTA delta targets in particular add real build time for artifacts nobody was going to flash.

`SSTATE_MIRRORS` is configured in `kas/base.yml`, pointing at a shared sstate cache. This mattered a lot across tonight's four real build runs: even a build that looked like a cold, from-scratch rebuild after a recipe change finished in a matter of minutes, not hours, because the overwhelming majority of BitBake's total task graph was an sstate-cache hit every time. The actual "Tasks Summary" lines from tonight's four builds, in order:

| Build | Tasks attempted | Didn't need to rerun | Result |
|---|---|---|---|
| 1 | 5902 | 5399 | **1 task failed** (the `aarch64`/`armv7` arch-mismatch QA failure, §4) |
| 2 | 6025 | 5945 | all succeeded |
| 3 | 6041 | 5968 | all succeeded |
| 4 | 6041 | 5985 | all succeeded |

Every build after the first attempted 6000+ tasks total but only had to actually *redo* on the order of a few dozen to a couple hundred of them — essentially just the new/changed custom recipes and anything downstream of them, with chromium, the kernel, Weston, and the rest of the dependency graph served straight from the sstate mirror. Only the very first build failed, and only on the one arch-mismatch QA check; every build after the fix succeeded outright.

## 7. Talking to a freshly-flashed device

After a flash (§8) and a normal reboot (unplug, then plug back in with **no** buttons held), the device enumerates over USB as a CDC-NCM network device — not `adb`, which is a stock-firmware-only access path that simply doesn't exist on this BSP. Bring the link up from the host:

```sh
sudo ip link set <enxIFACE> up
sudo dhcpcd -4 --timeout 10 <enxIFACE>
```

`dhcpcd` specifically, not `udhcpc`, `dhclient`, or `nmcli` — none of those were reliably present on the host doing this, `dhcpcd` was. The device runs the DHCP **server** role on this point-to-point link, which is counter-intuitive the first time: the address `dhcpcd` reports back as the *lease server* (its "leased X for Y from SERVER_IP" line) is the device's own IP address, not the host's.

SSH straight to that address as `root`. The kiosk image ships this wide open: **zero authentication**, not even a password — `ssh -v` shows `Authenticated to ... using "none"`. This is the shipped dev-image default, fine for bench work on a USB-tethered unit, but worth flagging clearly as something to lock down (e.g. `PermitRootLogin prohibit-password` plus a real key) before this device is ever anything but a desk toy.

`/root` lives on the read-only ext4 rootfs, which breaks both sides of a normal SSH key workflow: `ssh-keygen`'s default `~/.ssh` and plain `ssh`'s default `~/.ssh/known_hosts` both fail with "Read-only file system" for `root`. Redirect explicitly to the writable `/var` mount every time a key or `known_hosts` file is needed on-device:

```sh
mkdir -p /var/lib/thing-revived/.ssh
ssh-keygen -t ed25519 -N '' -f /var/lib/thing-revived/.ssh/id_ed25519 -q
ssh -i /var/lib/thing-revived/.ssh/id_ed25519 \
    -o UserKnownHostsFile=/var/lib/thing-revived/.ssh/known_hosts \
    -o StrictHostKeyChecking=accept-new ...
```

This BSP's BusyBox (v1.37.0 here) is stricter than GNU coreutils about flag syntax, and this applies broadly, not just to one command: it doesn't accept GNU long-option-style short flags (`head -10` fails; use `head -n 10`), and it has no `timeout` applet at all — bound a command another way (e.g. background it and `sleep` + `pkill -f <exact path>`) if you need one.

## 8. Flashing (Amlogic burn-mode) with `flashthing-cli`

The real flashing tool for this project is [`flashthing-cli`](https://crates.io/crates/flashthing-cli) (`cargo install flashthing-cli`) — **not** the older Python `superbird-tool`, which this project does not use (see the repo's own "no Python anywhere" rule).

- **Entering burn-mode** (hold buttons 1+4 while plugging in USB) and **the post-flash boot** (unplug, then plug back in normally with **no** buttons held) are both physical actions that only the human at the device can do. Never assume either happened — confirm burn-mode (or "not yet booted") via `lsusb | grep 1b8e:c003`; if that ID is gone and a `Multifunction Composite Gadget 1d6b:0104` appears instead, the device has already booted into the flashed OS.
- **First run** needs `sudo <full-path-to>/flashthing-cli --setup`, using the absolute path — plain `sudo flashthing-cli` fails because `sudo` doesn't inherit `~/.cargo/bin` on `PATH`. If it warns that `pkexec` isn't available, it prints the exact fallback udev-rule commands to run by hand (`sudo cp /tmp/98-superbird.rules /etc/udev/rules.d/ && sudo udevadm control --reload-rules && sudo udevadm trigger`) — just run what it printed, don't go troubleshooting `pkexec` itself.
- **A mid-flash `USB error: Operation timed out`** happened once tonight and is a real but recoverable failure mode, not corruption: the chip just drops back to its bare BootROM state (visibly different `bcdDevice` version and empty USB string descriptors compared to the BL2-stage's own descriptors once something is actually running) and waits for a fresh connection. The fix is simply rerunning `flashthing-cli <zip>` again — no need to make the human re-enter burn-mode by hand, bare BootROM still accepts the flash protocol directly.
- **`flashthing-cli` only writes the image; it never reboots into it.** The device sits back in mask-ROM mode after even a fully successful flash until the human does the physical unplug/replug.
- **What actually gets written**, as two separate `dd`-equivalent streams: a large sparse main user-area write (~1.43GB on tonight's builds) and a smaller second segment (~200MB) — roughly 1.5-1.6GB of real bytes landed on the eMMC each time. This is worth explicitly contrasting against the flashthing zip's own size, only ~345-363MB: the zip is sparse/compressed on disk, the ~1.5-1.6GB figure is the real decompressed footprint actually written to the device. Don't conflate the two numbers.

## 9. Live-patching vs. reflashing: which one actually persists, and when

At one point tonight, the running device's root filesystem was remounted read-write (`mount -o remount,rw /`) for a fast edit-test loop: changes pushed via `scp` and a `systemctl restart` landed as real writes to the actual `root_a` eMMC partition, not a tmpfs or overlay, so they did survive even without a reflash.

This is explicitly a **dev-convenience workaround, not the intended production mode**. The kernel command line mounts root `ro` by default on every normal boot; the remount only lasts until the next boot unless something else re-applies it. A reflash from the real, recipe-built image is still the only way to get a result that's reproducible and version-controlled, rather than a one-off hand-patched snapshot that exists only on one physical unit and nowhere in the repo's build definition.

There's also a quieter risk worth flagging: this BSP uses an A/B boot-slot scheme (`root=PARTLABEL=root_a` on the kernel cmdline implies a `root_b` fallback partition exists). A live-patched state sitting on the currently active slot could simply disappear with no error and no warning if the active slot ever flips to the other one — another reason a live patch is a convenience for iterating quickly, never a substitute for landing the change in a recipe and rebuilding.

## 10. Monitoring long builds without blocking

A background launcher shaped like:

```sh
nohup ssh devel "... &" & echo pid $!
```

reports completion almost immediately — that's the **local wrapper shell** exiting right after it forks the remote command on devel, not the real `docker run`/BitBake process actually finishing. Treat that notification as meaningless for build-progress purposes.

The reliable pattern is to find the actual `docker run` PID on the build host, then tie a monitor to that specific PID:

```sh
ssh devel "ps aux | grep 'docker run'"          # find the real PID
ssh devel "while kill -0 <real_pid> 2>/dev/null; do sleep 15; done"
```

Only that second command's own completion is trustworthy as a signal that the build actually finished.
