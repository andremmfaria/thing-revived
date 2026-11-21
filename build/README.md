# build/

Orchestration for producing and flashing a yocto-superbird kiosk image for this device. This directory does not vendor or commit any Yocto source/layer tree — the upstream [`yocto-superbird`](https://github.com/JoeyEamigh/yocto-superbird) repo is cloned fresh (or reused) at build time by `build-image.sh`, and only this project's own small overlay files get copied into that checkout before the real build runs. See `docs/yocto-build-and-flash.md` for the full narrative and the manual, step-by-step equivalent of everything these scripts automate.

## Layout

- `overlay/` — small text files only (recipes, `.service` units, a kernel config fragment, a `.bbappend`), laid out to mirror their exact destination paths inside the upstream checkout's existing `examples/meta-superbird-kiosk-example/` layer. No binaries and no full frontend copy live here; `build-image.sh` generates and places those at build time.
- `build-image.sh` — clones/reuses the upstream checkout, cross-compiles this repo's Rust agents, syncs `frontend/` and `overlay/` into the checkout, runs the real `kas-container build`, and stages the resulting flashthing zip into `artifact/`.
- `flash-image.sh` — flashes a built artifact zip to a physical device over USB in Amlogic burn-mode.
- `artifact/` — git-LFS-tracked flashthing zips (`*.zip`), one current snapshot at a time (see `.gitattributes`).

## Usage

### `build-image.sh`

```sh
build/build-image.sh
```

Environment variables:

- `YOCTO_CHECKOUT` (optional) — path to an existing yocto-superbird checkout to reuse (its sstate/downloads carry over between runs). If unset, a fresh `git clone --depth 1` is made into a unique `/tmp/yocto-superbird-build-$$` directory every run. If set to a path that doesn't exist yet, the clone is made there instead, so you can point it at a persistent location on a build host with real disk.

What it does, in order: clone/reuse the checkout → cross-compile `controller/` and `daemon/` for `aarch64-unknown-linux-gnu` with `+crt-static` → copy the two binaries into the checkout's `examples/meta-superbird-kiosk-example/recipes-extended/{controller-agent,thing-revived-terminal-agent}/files/` → sync this repo's `frontend/` into the webapp recipe's `files/` → copy `overlay/` on top of the checkout's example layer → run `kas-container build --target superbird-kiosk-prod-image kas/example-kiosk.yml` from inside the checkout → copy the resulting flashthing zip into `build/artifact/superbird-kiosk-prod-image-flashthing-YYYYMMDD.zip`.

Needs real disk (50-100GB+) and Docker for the `kas-container` step; don't run it on a disk-constrained host (see `docs/yocto-build-and-flash.md` §2).

### `flash-image.sh`

```sh
build/flash-image.sh [bus:device] [path/to/artifact.zip]
```

Both arguments are optional. With no arguments, it auto-detects the single USB device matching VID:PID `1b8e:c003` (Amlogic burn-mode) and flashes the newest zip in `artifact/`. Pass a specific `bus:device` (as shown by `lsusb`) to disambiguate when more than one matching device is present, and/or a specific zip path to flash something other than the newest build.

Entering burn-mode (hold buttons 1+4 while plugging in USB) and the post-flash boot (unplug, replug with no buttons held) are physical actions the script never performs — it only detects state and reports what's needed.

The script auto-retries a transient mid-write `USB error: Operation timed out` (the chip drops to bare BootROM and accepts a fresh connection immediately) up to twice with no human action needed. A wedged USB host controller (`USB error: Entity not found` persisting across retries, or repeated `xhci_hcd: Trying to add endpoint 0x81 without dropping it` in `dmesg`) is not software-recoverable — the script detects it, asks you to physically unplug/replug while holding burn-mode buttons again, and waits for the device to reappear before retrying.
