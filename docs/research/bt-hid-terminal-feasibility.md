# BT HID-Host Feasibility for a Device-Side Terminal

Research date: 2026-11-15. Scope: can a BT keyboard/mouse combo be paired TO the Car Thing (BlueZ HID-host role, not BT-PAN networking, not BLE peripheral/advertising mode) so a terminal UI can be driven by real keyboard input, and is an SSH client available to make that terminal useful against real hosts. All claims below are tagged with a confidence level; where evidence wasn't found, that is stated plainly instead of guessed.

This is a narrower, different question from `research.md`'s BT-PAN/NAP transport research: that document covers the Car Thing acting as a BT **network client** to reach a backend. This document covers the Car Thing acting as a BT **HID host**, accepting input from an external keyboard/mouse, the opposite direction.

## 1. Does yocto-superbird's BlueZ build restrict which plugins bluetoothd loads?

**Finding: no restriction found.** `meta-superbird/recipes-connectivity/bluez5/bluez5_%.bbappend` (fetched directly from the repo) only does two things: (a) applies a patch to make GATT LE-only, dropping BR/EDR ATT PSM-31 and GATT-over-BR/EDR SDP records, to fix an iOS AMS/ANCS bonding issue, and (b) sed-patches `/etc/bluetooth/main.conf` to set `Class = 0x7c0000`, `Experimental = true`, `KernelExperimental = true`. It does not touch bluetoothd's `ExecStart` line and does not add any `--plugin=`/`--noplugin=` flag or reference HID/input anywhere. The standard oe-core bluez5 recipe's systemd unit ships `ExecStart=/usr/libexec/bluetooth/bluetoothd` with no plugin whitelist, meaning bluetoothd loads **all** compiled-in plugins by default on this BSP.

This is a materially different posture from stock Spotify firmware, where `ps aux` showed `bluetoothd --plugin=gap,deviceinfo` explicitly, a Spotify-side restriction that excludes the `input` plugin (BlueZ's HID-host driver). That restriction is not present in yocto-superbird.

**Confidence: medium-high.** Directly confirmed by reading the actual bbappend file content. Not verified against a built image's resulting `/lib/systemd/system/bluetooth.service`, and no bbappend overriding bluez5's own systemd unit was found in the repo tree, which supports "no override exists" but isn't a built-and-booted confirmation.

Sources:
- https://raw.githubusercontent.com/JoeyEamigh/yocto-superbird/main/meta-superbird/recipes-connectivity/bluez5/bluez5_%.bbappend
- https://github.com/JoeyEamigh/yocto-superbird/tree/main/meta-superbird/recipes-connectivity/bluez5

## 2. Generic mechanism: does a paired HID keyboard/mouse surface as a normal evdev node?

**Finding: confirmed generically.** BlueZ's `input` plugin is the HID-host driver; once a BT Classic or BLE (HID-over-GATT) HID device bonds, bluetoothd opens `/dev/uhid` and the kernel's `hid-generic`/uhid path creates a normal `/dev/input/eventN` node, identical in form to a USB HID device. A real `bluez/bluez` GitHub issue log shows exactly this: `hid-generic ...: input,hidraw1: BLUETOOTH HID ... Mouse` followed by a `/dev/input/eventN` creation. Linux's evdev layer is transport-agnostic by design, so this project's existing Rust bridge daemon (uses the `evdev` crate to read the device's own physical buttons/knob today) would need **no BT-specific code** to also read a paired external keyboard, same enumeration/open/read path.

**Caveat found:** a 2025 Sailfish OS bug report documents a real failure mode where bluetoothd could not open `/dev/uhid` due to device-node permissions/hardening; the HID device bonded successfully but no evdev node appeared until the permission issue was fixed. This is a config precondition to check on this BSP, not a fundamental limitation of the mechanism.

**Confidence: high** on the general mechanism; **medium** on the permission caveat (one concrete report, plausible and relevant, not yet checked against this specific BSP).

Sources:
- https://github.com/bluez/bluez/issues/1886
- https://docs.kernel.org/input/input.html
- https://www.joeshaw.org/linux-input-ecosystem
- https://forum.sailfishos.org/t/5-2-0-16-hid-over-gatt-devices-connect-but-no-input-device-is-created-bluetoothd-cannot-open-dev-uhid/31407

## 3. BCM20703A2-specific HID-host quirks

**Finding: none found.** Searches surfaced only generic BlueZ HID issues unrelated to this chip (various other chipsets, e.g. vendor IDs 0x0A5C, 0x046D) and generic CapsLock/reconnect race-condition threads on linux-input/linux-bluetooth mailing lists, none naming Broadcom/Cypress 2070x-family chips. BCM20703A2 is a common combo chip (same family used in many laptops and Raspberry Pi variants) with mature Broadcom/`btbcm` Linux driver support.

**Confidence: unconfirmed, no evidence either way.** This is absence of a negative report, not a positive confirmation of clean HID-host operation on this exact chip.

## 4. SSH client availability

**Finding:** the top-level yocto-superbird README states `superbird-bsp-image` ships "kernel, busybox, sshd, USB-CDC gadget", confirming an SSH **server** is shipped by default. An `openssh` bbappend/layer customization already exists in the BSP (`meta-superbird/recipes-connectivity/openssh/openssh_%.bbappend`, confirmed present via directory listing; raw content fetch failed, likely a `%`-in-filename URL-encoding issue, not re-attempted). The README does not explicitly list an SSH **client** binary as shipped. Standard oe-core's `openssh` recipe splits into separate packages (client `ssh`/`scp` vs. `openssh-sshd`), so having sshd does not automatically pull the client. Since the openssh recipe/layer is already present and used in this BSP, adding the client is expected to be a one-line `IMAGE_INSTALL:append` change (`openssh-ssh`, or `dropbear`, both standard oe-core/meta-openembedded recipes), no new layer needed.

**Confidence: high** that sshd is shipped; **medium** (not directly confirmed) on whether the client binary ships by default; **high** that it is trivially addable if not.

Sources:
- https://github.com/JoeyEamigh/yocto-superbird (README)
- https://github.com/JoeyEamigh/yocto-superbird/tree/main/meta-superbird/recipes-connectivity/openssh
- https://ejaaskel.dev/adding-key-based-ssh-authentication-to-yocto (generic openssh/dropbear packaging pattern, not device-specific)

## 5. Prior community precedent for pairing an external keyboard/mouse to a Car Thing

**Finding: none found.** r/carthinghax and r/truespotify discussions of Car Thing Bluetooth are exclusively about the Car Thing acting as a BT **audio sink** (pairing it to a phone/stereo) or CarPlay/Android-auto-adjacent hacks, not about pairing an external keyboard or mouse to it as an input peripheral. No DeskThing, superbird-tool, or community Discord post surfaced describing this use case.

**Confidence: explicitly unconfirmed.** This is "not found," not "confirmed untested" or "confirmed broken." Absence of evidence, not evidence of absence.

## Overall verdict

Enabling BT keyboard/mouse pairing and getting it to surface as a normal evdev node, pickable up by the existing Rust bridge daemon's `evdev`-based reader with no new code, is **likely to work**, conditional on two facts partially confirmed and one not confirmed at all:

- yocto-superbird's bluez5 build does not restrict bluetoothd's plugin list the way stock Spotify firmware does, so the `input` plugin should load by default. This is the single most de-risking fact found here, a direct reading of the actual BSP config, not an inference from stock firmware's different, more restrictive setup.
- The generic BlueZ/kernel HID-host-to-evdev pathway is standard, well-documented, transport-agnostic, and already how this project's own evdev-based bridge daemon design works for the device's physical controls.
- No BCM20703A2-specific blocker and no community precedent either way were found. The chip is common enough that a hidden showstopper seems unlikely but is not ruled out.

**Single thing needing live on-device verification before committing to implementation:** boot a yocto-superbird image, bond a real BT keyboard, confirm (a) the `org.bluez.Input1` interface appears on the bonded device (via `bluetoothctl` or D-Bus introspection), and (b) a new `/dev/input/eventN` node actually appears and emits `EV_KEY` events on keypress (via `evtest` if present, or the bridge daemon's own `evdev` enumeration). This resolves the uhid-permission caveat from Section 2 empirically rather than by further literature review.

## Implication for a terminal UI (separate design note, not researched above)

If the HID-host path is confirmed, the practical path to an on-device terminal does not need a classic Linux VT console (which would contend with Qt EGLFS/Chromium for DRM ownership). Instead: run `xterm.js` inside the existing Chromium kiosk (same no-build-step static-page pattern already proven for the dashboard and bridge/UI prototype), backed by a PTY, with the paired keyboard's evdev events relayed to it over the bridge daemon's existing local WebSocket, the same transport already used for button/knob events. The PTY can run a local shell or, once an SSH client is confirmed/added per Section 4, `ssh` out to a real host (devel/preto/vermelho), turning the device into an actual remote terminal rather than a toy local shell. The device's 5 physical buttons and knob remain available as session chrome (scroll, session switch, interrupt, clear) layered on top of real keyboard typing, reusing the bridge daemon's existing translation layer, not a new input pipeline.
