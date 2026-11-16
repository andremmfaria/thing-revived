<p align="center">
  <img src="docs/img/logo-wordmark.svg" alt="thing-revived" width="420">
</p>

Turning a discontinued Spotify "Car Thing" into a standalone Bluetooth-connected physical control surface for the smart-home controller: its knob, buttons, and touchscreen driving real automation actions, with a web UI on the device's own screen.

**Start here: [`docs/overview.md`](docs/overview.md)** (map of every other doc in this repo). Don't duplicate facts across files; if something needs updating, update it in the one doc that owns it (linked from the overview) and let everything else reference that. For what's actually implemented and running today, see [`docs/development.md`](docs/development.md).

## Quick links

- [Overview](docs/overview.md)
- [Current state (what's actually running)](docs/development.md)
- [Architecture](docs/architecture.md)
- [Hardware reference](docs/hardware.md)
- [Getting started / running the prototype](docs/getting-started.md)
- [Device internals dashboard](docs/dashboard.md)
- [On-device terminal](docs/terminal.md)
- [Dependency surface](docs/dependency-surface.md)
- [yocto-superbird build & flash workflow](docs/yocto-build-and-flash.md)
- [Full research trail](docs/research/)

## Layout

```
terminal/                     Terminal UI backed by a PTY agent (agent/ + frontend/)
prototype/                    Early prototypes; see prototype/README.md
  control-surface/            Rust daemon (agent/) + React UI (frontend/)
  dashboard/                  NOC-style dashboard (agent/ + frontend/)
docs/                          Project documentation (see docs/overview.md)
```

For the current status of each of these (which is the main active project, what's deployed vs.
prototype-only, what's verified on real hardware), see [`docs/development.md`](docs/development.md).
