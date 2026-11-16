# prototype/

This directory holds the repo's two **initial prototypes**:

- [`control-surface/`](control-surface/) — the first prototype built: a Rust bridge daemon (`agent/`) relaying hardware button/knob input events to a React UI (`frontend/`) over a local WebSocket.
- [`dashboard/`](dashboard/) — a NOC-style dashboard prototype showing the device's own CPU, memory, disk and I/O (`agent/` + `frontend/`).

Both were originally top-level project directories in this repo. They were moved here to separate them, in the repo's structure and narrative, from [`terminal/`](../terminal/), which is the main active project going forward. The move was a reorganization only — no functional changes, no renaming of the agent/frontend/deploy internals, and no effect on anything already built or flashed onto the physical device (yocto recipes reference prebuilt binary artifacts by filename, not by this repo's directory layout).

See [`docs/overview.md`](../docs/overview.md) for the full project map, and the relevant doc for each prototype: [`docs/getting-started.md`](../docs/getting-started.md) (control-surface) and [`docs/dashboard.md`](../docs/dashboard.md) (dashboard).
