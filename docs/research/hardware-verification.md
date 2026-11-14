# On-Device Verification — Live ADB Shell (2026-11-14)

Performed against the physical unit, serial `8559R486QR1S`, via `adb shell` as root after
adding a udev rule for `1d6b:1014` (vendor-specific ADB interface class 0xFF/0x42/0x01 was
already exposed by the firmware over USB in its current boot state — no burn-mode/button
combo was needed to get this access). This replaces every "unconfirmed, verify on-device"
item from research.md §1 and §8 with real data. Firmware on this unit is stock Spotify —
confirmed NOT yet re-flashed with any community image (nixos-superbird / yocto-superbird /
superbird-debian-kiosk).

**CORRECTION (ps aux pass, below): the initial `find` for Chromium came back empty because
it was depth-limited to 4 and Chromium lives under `/usr/bin/chromium-browser/`. Stock
firmware IS running Chromium.** This directly resolves webapp-feasibility.md's unadjudicated
"secondhand claim that stock firmware moved to Chromium" — that claim was correct. The
`/etc/qt-superbird-app` directory is the native launcher/supervisor's config, not the whole
UI stack as originally assumed.

## OS / userland (confirms §8, upgrades inferred → confirmed)

- `/etc/os-release`: `Buildroot`, `VERSION_ID=2019.02.1` (tag), `VERSION=2019.02.1-g841943219d-dirty`
- `uname -a`: `Linux buildroot 4.9.113 #1 SMP PREEMPT Mon May 4 04:20:20 UTC 2020 aarch64`
- **BusyBox CONFIRMED by name** (was inferred-only in §8): `/bin/busybox` → `BusyBox v1.36.0 (2023-04-24)`.
  Note: busybox build date (2023) is newer than the Buildroot release tag (2019) — this unit's
  firmware received later OTA patches without bumping the base os-release string.
- `getprop: not found` — confirms no Android property service / init.rc, consistent with §8's
  "Buildroot, not Android" verdict.
- PID 1 is literally `init` (not systemd, not Android's `init`), consistent with the SysV/BusyBox
  init.d pattern already documented in §8.
- `/etc/qt-superbird-app` exists — direct confirmation (not inference) of the Qt app runtime.
- No `chromium`/`libQt` hits elsewhere in a filesystem-wide find at depth 4 — this specific
  firmware build is pure native Qt, no embedded browser engine anywhere.

## Input devices (fully resolves §1.1's "unconfirmed event codes" gap)

```
/proc/bus/input/devices:

event0  "gpio-keys"      EV=3 (SYN+KEY)   KEY bitmap 40000 1000003e   -> the 5 physical buttons
                                                                          (+ possibly knob click,
                                                                          bitmap has room for 6 keys)
event1  "rotary@0"       EV=5 (SYN+REL)   REL bitmap 40               -> the knob's ROTATION,
                                                                          reported as a relative
                                                                          axis event (bit 6 set =
                                                                          REL_HWHEEL in the standard
                                                                          Linux input event-code
                                                                          table), NOT discrete
                                                                          KEY_LEFT/KEY_RIGHT as some
                                                                          generic rotary-encoder
                                                                          drivers do
event2  "aml_vkeypad"    EV=3 (SYN+KEY)   KEY bitmap 100000           -> Amlogic virtual keypad,
                                                                          single bit set, likely one
                                                                          unrelated system key
                                                                          (needs a value dump to
                                                                          name it; low priority)
event3  "tlsc6x_dbg"     EV=b (SYN+KEY+ABS) ABS bitmap present, I2C   -> the TOUCHSCREEN. "tlsc6x"
                                                                          names the controller
                                                                          family (TLSC/FocalTech-style
                                                                          capacitive touch IC) on
                                                                          I2C bus 0, address 0x2e.
                                                                          Confirmed I2C, standard
                                                                          Linux multitouch evdev
                                                                          device — resolves §1's
                                                                          "I2C vs SPI, unconfirmed"
                                                                          question: it's I2C.
```

**Still open, needs one more pass with the device live and buttons physically pressed**: capture
`evtest`/`getevent -lt` output while actually pressing each of the 5 buttons and clicking/turning
the knob, to map exact scancodes to physical positions and confirm whether knob-press is in
`event0`'s bitmap or somewhere else. The event *nodes and type* are now confirmed; the exact
*scancode-to-physical-button* mapping is not yet captured.

## GPU / display (resolves the yocto-superbird GPU open question for THIS firmware)

- `/dev/dri/card0` + `/dev/dri/controlD64` present → kernel DRM/KMS is active.
- `/dev/mali0` present → the vendor Mali kernel driver/blob is loaded and exposed, not just
  theoretically available. This firmware's Qt+EGLFS stack has a real GPU device node to render
  through, consistent with §8's claim and webapp-feasibility.md's Panfrost-capability finding —
  though this specific node confirms the **vendor blob** path is what's active here, not Panfrost
  (Panfrost would be a mainline-kernel DRM driver choice made by a *different* firmware, e.g.
  yocto-superbird, not something present on this stock-ish build).

## Storage partitions (confirms §8's AOSP-style A/B layout claim with a real dump)

18 numbered partitions on `mmcblk0` (plus `boot0`/`boot1`/`rpmb` eMMC hardware partitions) —
sizes consistent with a bootloader/env/dtbo/vbmeta/boot/system A-B scheme plus a large (~2.2GB)
final partition that is almost certainly `/data`. Exact name-to-partition-number mapping not
pulled yet (would need `/proc/cmdline` or the partition GUID/label table, not just `/proc/partitions`
sizes) — flagged for a follow-up pass, not fabricated here.

## Bluetooth (materially changes the project plan)

- `ls /sys/class/bluetooth` → `hci0` is present and enumerated **right now, on this stock-ish
  firmware, with no community re-flash at all**. The BT controller is recognized at the kernel/BlueZ
  HCI level out of the box.
- This is a stronger starting position than research.md's §3 assumed: BT-PAN/RFCOMM experimentation
  can potentially begin against the CURRENT firmware (verify `bluetoothctl` / `hciconfig` actually
  bring the radio up and pair, as a next step), rather than requiring a full yocto-superbird
  re-flash just to get a working BT stack first.

## Live process list + real RAM measurement (ps aux + free -m, stock firmware, fully booted)

```
free -m:        total 488, used 152, free 169, buff/cache 166, available 323
/proc/meminfo:  MemTotal 499928kB, MemAvailable 330448kB

Key processes actually running right now, stock firmware, full normal operation:
  qt-superbird-app          (native launcher/supervisor, user "superbir", 1h40 CPU)
  chromium-browser/chrome   --kiosk --no-sandbox --in-process-gpu --remote-debugging-port=2222
                             --app=file:///usr/share/qt-superbird-app/webapp/index.html
                             (1h07 CPU — THE ACTUAL UI IS A LOCAL HTML/JS WEB APP RENDERED
                              BY CHROMIUM, launched/supervised by the native Qt process, not
                              a pure-QML app as the initial filesystem scan wrongly suggested)
    + chrome --type=zygote
    + chrome --type=renderer --num-raster-threads=2 ...
    + chrome --type=utility --service-sandbox-type=audio
  bluetoothd                 --plugin=gap,deviceinfo (confirms hci0 is a live, used radio)
  earlyoom                   -m 1 --prefer '(.*chrome.*)' (OOM policy explicitly tuned
                              around Chromium being the first thing to sacrifice under
                              pressure — but it's still the shipped default UI)
  swupdate                    -k /etc/spotify-public.pem (OTA update agent)
  sp-als-backlight             (ambient-light-sensor-driven backlight daemon)
```

**This overturns the "avoid Chromium/QtWebEngine" framing from architecture-decision.md's
first pass.** Stock Spotify production firmware already ships exactly a "Chromium kiosk
rendering a local web app, supervised by a thin native launcher" architecture, remote-debug
port open (2222) and all, on this exact 488MB unit — and real measured total system RAM used
is 152MB with everything (full Chromium process tree + BT stack + OOM daemon + updater +
backlight service) running simultaneously, leaving 323MB available. The `--in-process-gpu`
flag is notable: it folds the GPU process into the main browser process, one fewer process +
IPC hop than default Chromium, and confirms the Mali blob is used for Chromium's own
acceleration too, not just Qt/EGLFS elsewhere.

**Still true and unaffected by this correction:** this is the vendor Mali blob, not Panfrost;
BT hci0 live; input event nodes as mapped above; BusyBox confirmed; Buildroot kernel
4.9.113. Only the "Chromium absent, pure-QML-only" part of the initial scan was wrong — fixed
above and flagged for architecture-decision.md to re-evaluate against this real measurement.

## Prototype build pass corrections (2026-11-14, Craftsman session)

Building and running the first bridge+UI prototype on the real unit surfaced three corrections
to assumptions made in this doc and in `architecture-decision.md`/`webapp-feasibility.md`:

1. **Userspace is 32-bit ARM (`armhf`), not aarch64, despite the 64-bit kernel.** `uname -a`
   reports `aarch64` for the *kernel*, but `/lib/ld-linux-armhf.so.3` and a failed
   `aarch64-unknown-linux-gnu` binary confirm the actual userspace ABI is ARMv7 hard-float —
   a 32-bit-userland-on-64-bit-kernel setup, presumably to save RAM. **Any native binary built
   for this device must target `armv7-unknown-linux-gnueabihf` (or equivalent 32-bit ARM
   triple), not aarch64.** This corrects every prior doc that assumed aarch64 native builds.
2. **On-device glibc (~2.30) is older than typical cross-toolchain output.** A normally-built
   Rust binary failed at runtime with missing `GLIBC_2.32/2.33/2.34` symbols. Fix used:
   `RUSTFLAGS="-C target-feature=+crt-static"` for a fully static binary with zero libc
   dependency — recommended default for any future native binary built for this device,
   sidesteps the glibc-version-matching problem entirely.
3. **The unit is NOT running unmodified stock Spotify firmware — it's running DeskThing
   (community custom client) on top of the stock launcher architecture.** CDP's
   `navigator.userAgent`/page title identify on-device Chromium as version 69 (2018) and the
   active page as "DeskThing Client", not Spotify's own webapp. This means the earlier
   `ps aux` finding (`qt-superbird-app` launching `chromium-browser/chrome --app=file:///usr/share/qt-superbird-app/webapp/index.html`)
   was genuine, but the *content* being loaded by that launcher had already been swapped to
   DeskThing's client at some point before this session — not a contradiction of the launcher
   architecture itself, just evidence someone (possibly the user, previously) already
   replaced the webapp payload without touching the native launcher/OS layer. Good news,
   functionally: it proves the "swap the webapp, keep the launcher/Chromium/OS as-is" approach
   this project also wants to use is already a validated pattern on this exact unit.
   Implication for the UI build: **target old Chromium 69 explicitly** (e.g. Vite's
   `build.target: 'chrome69'`) — modern JS syntax (`??`, optional chaining `?.`) silently
   fails to execute on this engine with no console error, just a React app that never mounts.
4. `evtest`/`getevent` are **not present** on this unit — there is no on-device tool for a
   root-shell-only live-press capture; the scancode mapping must be captured by watching the
   bridge daemon's own output while a human physically presses each control.

## What this changes vs. prior docs

1. BusyBox: inferred → **confirmed**.
2. Input event codes: "unconfirmed, verify with evtest" → **node/type confirmed** (button vs.
   knob-rotation vs. touch, and touch bus is I2C); exact scancode-per-button still pending a
   live-press capture.
3. GPU: "Panfrost capability exists on this SoC family" (webapp-feasibility.md) is about the
   SoC in general; THIS unit's current firmware uses the **vendor Mali blob** via `/dev/mali0`,
   not Panfrost — don't conflate the two when planning the custom-firmware GPU path.
4. Bluetooth: `hci0` already live on stock-ish firmware — re-flashing to yocto-superbird purely
   to "get BT" may not be necessary; worth testing BT pairing against current firmware before
   committing to a re-flash.
