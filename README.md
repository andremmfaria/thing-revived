# superbird-ha-bridge

Turning a discontinued Spotify "Car Thing" into a standalone Bluetooth-connected physical
control surface for Home Assistant — its knob, buttons, and touchscreen driving real HA
actions, with a web UI on the device's own screen.

**Start here: [`docs/overview.md`](docs/overview.md)** — current state and a map of every
other doc in this repo. Don't duplicate facts across files; if something needs updating,
update it in the one doc that owns it (linked from the overview) and let everything else
reference that.

## Quick links

- [Overview](docs/overview.md)
- [Architecture](docs/architecture.md)
- [Hardware reference](docs/hardware.md)
- [Getting started / running the prototype](docs/getting-started.md)
- [Full research trail](docs/research/)

## Layout

```
bridge/   Rust daemon: hardware input (evdev) + Bluetooth + Home Assistant client
ui/       React UI, rendered by the device's on-board Chromium kiosk
docs/     Project documentation (see docs/overview.md)
```
