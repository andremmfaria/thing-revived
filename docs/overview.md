# Project Overview

Repurposing a discontinued Spotify "Car Thing" (community codename **superbird**) into a standalone, Bluetooth-connected physical control surface for the smart-home controller: its rotary knob, 5 buttons, and touchscreen driving real automation actions, with a React UI on the device's own screen.

This file is the entry point. It gives the current state and links out to the detailed write-ups rather than repeating them: each fact below lives in exactly one place.

## Current state

A working proof-of-life prototype runs on the physical unit: pressing hardware controls is designed to produce a visible on-screen reaction, via a Rust bridge daemon relaying raw input events to a React UI over a local WebSocket. See [`getting-started.md`](getting-started.md) to run it, and [`research/hardware-verification.md`](research/hardware-verification.md) for what's been confirmed on real hardware vs. what's still open (notably: exact button/knob scancodes need a live capture pass).

## Where things live

- [`architecture.md`](architecture.md): the chosen system design and why, with links into the full reasoning trail.
- [`hardware.md`](hardware.md): device facts (SoC, inputs, GPU, OS, RAM) at a glance.
- [`getting-started.md`](getting-started.md): build/run instructions for `bridge/` and `ui/`.
- [`dashboard.md`](dashboard.md): a NOC-style dashboard showing the device's own CPU, memory, disk and I/O, separate from the control-surface prototype above.
- [`dependency-surface.md`](dependency-surface.md): what this project actually depends on to run, correcting two wrong assumptions (not tied to yocto-superbird, not tied to DeskThing) and laying out the one real remaining tradeoff.
- [`research/`](research/): the full research trail, in the order it was produced:
  - [`research.md`](research/research.md): hardware deep dive, firmware/community-project landscape, Bluetooth transport design, smart-home controller command-layer options.
  - [`webapp-feasibility.md`](research/webapp-feasibility.md): what can run on-device, cross-compilation questions, candidate UI architectures.
  - [`hardware-verification.md`](research/hardware-verification.md): real `adb` shell findings on the physical unit (OS, input devices, GPU, RAM, running processes), including corrections made during the first prototype build.
  - [`architecture-decision.md`](research/architecture-decision.md): the architecture decision record, including a same-day revision once real RAM data came in.
  - [`hardware-interaction.md`](research/hardware-interaction.md): exactly how a physical button/knob press reaches the smart-home controller and the on-device UI under the chosen architecture, and the bridge daemon's implementation-language choice.

## Non-goals (for now)

No smart-home controller integration and no Bluetooth transport yet. The current prototype is input-hardware-to-screen only, by design, to de-risk the pipeline before adding networking.
