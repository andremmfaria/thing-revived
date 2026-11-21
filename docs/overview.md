# Project Overview

Repurposing a discontinued Spotify "Car Thing" (community codename **superbird**) into a standalone, Bluetooth-connected physical control surface for the smart-home controller: its rotary knob, 5 buttons, and touchscreen driving real automation actions, with a React UI on the device's own screen.

This file is the entry point. It links out to the detailed write-ups rather than repeating them: each fact below lives in exactly one place. For what's actually implemented and running today, see [`development.md`](development.md).

## Where things live

- [`development.md`](development.md): what's actually implemented and running today, across `terminal/` and `controller/` (the former `prototype/dashboard/` was removed entirely, never deployed, no replacement).
- [`architecture.md`](architecture.md): the chosen system design and why, with links into the full reasoning trail.
- [`hardware.md`](hardware.md): device facts (SoC, inputs, GPU, OS, RAM) at a glance.
- [`getting-started.md`](getting-started.md): build/run instructions for `controller/agent/` and `controller/frontend/`.
- [`dashboard.md`](dashboard.md): the former NOC-style metrics dashboard prototype, removed entirely (never deployed, no replacement).
- [`dependency-surface.md`](dependency-surface.md): what this project actually depends on to run; see the REVISION note at the top — the device has since been reflashed to yocto-superbird, superseding that file's original "never used" finding.
- [`terminal.md`](terminal.md): a real, vendored xterm.js + FitAddon terminal UI backed by a PTY agent, driven by a Bluetooth-paired keyboard plus the device's buttons/knob for session chrome; verified on real hardware 2026-11-15 (see that file's own section of that name).
- [`yocto-build-and-flash.md`](yocto-build-and-flash.md): the full yocto-superbird build, customization, and flashing workflow — why this BSP was chosen over stock firmware, the build environment, the three recipe-level image customizations made, building and monitoring the build, talking to a freshly-flashed device, the flashing procedure itself, and live-patching vs. reflashing as an operational distinction.
- [`research/`](research/): the full research trail, in the order it was produced:
  - [`research.md`](research/research.md): hardware deep dive, firmware/community-project landscape, Bluetooth transport design, smart-home controller command-layer options.
  - [`webapp-feasibility.md`](research/webapp-feasibility.md): what can run on-device, cross-compilation questions, candidate UI architectures.
  - [`hardware-verification.md`](research/hardware-verification.md): real `adb` shell findings on the physical unit (OS, input devices, GPU, RAM, running processes), including corrections made during the first prototype build.
  - [`architecture-decision.md`](research/architecture-decision.md): the architecture decision record, including a same-day revision once real RAM data came in.
  - [`hardware-interaction.md`](research/hardware-interaction.md): exactly how a physical button/knob press reaches the smart-home controller and the on-device UI under the chosen architecture, and the bridge daemon's implementation-language choice.
  - [`bt-hid-terminal-feasibility.md`](research/bt-hid-terminal-feasibility.md): feasibility research for pairing a BT keyboard/mouse to the device as input peripherals (BlueZ HID-host role) to drive an on-device terminal, plus SSH client availability on yocto-superbird.
  - [`bash-availability.md`](research/bash-availability.md): whether a real bash shell is available or addable (vs. BusyBox ash) for the on-device terminal's PTY backend, across stock firmware and the three community firmware options.
  - [`competitor-features.md`](research/competitor-features.md): a feature catalogue of Nocturne and DeskThing, the two most relevant alternative-firmware projects for this device, used as a grounded idea pool for thing-revived's own roadmap without adopting either project's architecture.

## Non-goals (for now)

No smart-home controller integration and no Bluetooth transport yet. The current prototype is input-hardware-to-screen only, by design, to de-risk the pipeline before adding networking.
