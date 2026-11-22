# Project Overview

thing-revived revives the discontinued Spotify "Car Thing" (community codename **superbird**) as a standalone on-device terminal and settings surface. A custom Yocto image boots straight into a Chromium 69 kiosk on the device's own screen. The knob, buttons and touchscreen drive a terminal UI and a settings menu (Bluetooth, brightness, timezone, Wi-Fi), and a paired Bluetooth keyboard types into the terminal. The device clock syncs over NTP once it has network.

This file is the entry point. It links out to the detailed write-ups rather than repeating them: each fact below lives in exactly one place. The top-level [`README.md`](../README.md) has the quick start and repository layout.

## Where things live

- [`development.md`](development.md): what's actually implemented and running on the device today, across `daemon/`, `frontend/`, and `controller/`.
- [`architecture.md`](architecture.md): the chosen system design and why, with links into the full reasoning trail.
- [`hardware.md`](hardware.md): device facts (SoC, inputs, GPU, OS, RAM, access) at a glance.
- [`yocto-build-and-flash.md`](yocto-build-and-flash.md): the yocto-superbird build and flashing workflow, the layers/overlay this project adds, and the device-access facts for a freshly-flashed unit.
- [`wifi.md`](wifi.md): Wi-Fi in the settings menu, which works on real hardware through a USB adapter in host mode (verified with a D-Link DWA-171 on a USB-C dock; dock Ethernet also works).
- [`terminal.md`](terminal.md): the on-device terminal feature: a vendored xterm.js + FitAddon UI backed by a PTY agent, driven by a Bluetooth-paired keyboard plus the device's buttons/knob for session chrome.
- [`dependency-surface.md`](dependency-surface.md): what this project actually depends on to run.
- [`coming-soon.md`](coming-soon.md): an unscheduled backlog of ideas, not a roadmap.
- [`research/`](research/): the full research trail, in the order it was produced:
  - [`research.md`](research/research.md): hardware deep dive, firmware/community-project landscape, Bluetooth transport design, smart-home controller command-layer options.
  - [`webapp-feasibility.md`](research/webapp-feasibility.md): what can run on-device, cross-compilation questions, candidate UI architectures.
  - [`hardware-verification.md`](research/hardware-verification.md): real shell findings on the physical unit (OS, input devices, GPU, RAM, running processes), including corrections made during the first prototype build and the stock-firmware baseline this project moved on from.
  - [`architecture-decision.md`](research/architecture-decision.md): the architecture decision record, including a same-day revision once real RAM data came in.
  - [`hardware-interaction.md`](research/hardware-interaction.md): exactly how a physical button/knob press reaches the on-device UI under the chosen architecture, and the bridge daemon's implementation-language choice.
  - [`bt-hid-terminal-feasibility.md`](research/bt-hid-terminal-feasibility.md): feasibility research for pairing a BT keyboard/mouse to the device as input peripherals (BlueZ HID-host role) to drive an on-device terminal, plus SSH client availability on yocto-superbird.
  - [`bash-availability.md`](research/bash-availability.md): whether a real bash shell is available or addable (vs. BusyBox ash) for the on-device terminal's PTY backend, across stock firmware and the three community firmware options.
  - [`competitor-features.md`](research/competitor-features.md): a feature catalogue of Nocturne and DeskThing, the two most relevant alternative-firmware projects for this device, used as a grounded idea pool for thing-revived's own roadmap without adopting either project's architecture.
  - [`button-knob-clock-design.md`](research/button-knob-clock-design.md): the button/knob session-switching design and the bridge daemon's `EVIOCGRAB`+`uinput` mechanism.
  - [`terminal-hardware-verification.md`](research/terminal-hardware-verification.md): real-hardware verification history for the terminal feature.
  - [`yocto-build-decisions.md`](research/yocto-build-decisions.md): the build/flash tooling decisions and troubleshooting history behind the yocto-superbird workflow.
  - [`settings-menu-feasibility.md`](research/settings-menu-feasibility.md): feasibility and design of the settings menu.
  - [`bt-peripheral-pairing-workflow.md`](research/bt-peripheral-pairing-workflow.md): the Bluetooth pairing workflow design.
  - [`dwa-171-wifi.md`](research/dwa-171-wifi.md): D-Link DWA-171 Wi-Fi support on the superbird kernel.
  - [`lmp-mini-dock-chipsets.md`](research/lmp-mini-dock-chipsets.md): chipset research for a USB-C mini dock.
  - [`promotion-venues.md`](research/promotion-venues.md): where the project could be promoted.

## Non-goals

No smart-home or Bluetooth-peripheral control. This was the project's original premise but was cut from scope in favor of the terminal and settings menu being the deliverable.
