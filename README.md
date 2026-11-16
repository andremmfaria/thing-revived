<p align="center">
  <img src="docs/img/logo-wordmark.svg" alt="thing-revived" width="420">
</p>

Turning a discontinued Spotify "Car Thing" into a standalone Bluetooth-connected physical control surface for the smart-home controller: its knob, buttons, and touchscreen driving real automation actions, with a web UI on the device's own screen.

**Start here: [`docs/overview.md`](docs/overview.md)** (current state and a map of every other doc in this repo). Don't duplicate facts across files; if something needs updating, update it in the one doc that owns it (linked from the overview) and let everything else reference that.

## Quick links

- [Overview](docs/overview.md)
- [Architecture](docs/architecture.md)
- [Hardware reference](docs/hardware.md)
- [Getting started / running the prototype](docs/getting-started.md)
- [Device internals dashboard](docs/dashboard.md)
- [yocto-superbird build & flash workflow](docs/yocto-build-and-flash.md)
- [Full research trail](docs/research/)

## Layout

```
terminal/                     ACTIVE PROJECT — xterm.js-placeholder terminal UI backed by a PTY agent (agent/ + frontend/)
prototype/                    Early prototypes, kept for history/reference; see prototype/README.md
  control-surface/            Rust daemon (agent/, hardware input via evdev + Bluetooth + smart-home controller client) + React UI (frontend/, rendered by the device's on-board Chromium kiosk)
  dashboard/                  NOC-style dashboard showing the device's own CPU, memory, disk and I/O (agent/ + frontend/)
docs/                          Project documentation (see docs/overview.md)
```
