# config/

One TOML file per component. Each file is fully commented and its values equal the defaults compiled into the component, so deleting a key (or the whole file) never changes behaviour.

| File | Consumed by | Where it ends up on the device | Env override |
|---|---|---|---|
| `controller.toml` | controller-agent (`controller/`) | `/etc/thing-revived/controller.toml` | `CONTROLLER_CONFIG` |
| `terminal.toml` | terminal-agent (`daemon/`), including the `[[session]]` tables that replace the old `terminal-agent-config.json` | `/etc/thing-revived/terminal.toml` | `TERMINAL_AGENT_CONFIG` |
| `frontend.toml` | the kiosk web app (`frontend/`), via the generator below | `/usr/share/superbird-kiosk-default/config.generated.js` | none |

## protocol/

`config/protocol/` holds the golden JSON wire fixtures (one file per message variant) shared by the controller and daemon Rust tests and by the frontend node and e2e tests. It is not read at runtime and is not installed on the device. See `config/protocol/README.md`.

## How each file is consumed

Agents (controller, terminal): the agent reads the path in its env var, falling back to `/etc/thing-revived/<name>.toml`. If the file is missing it runs on compiled-in defaults. If the file exists but is invalid it exits with status 78 (EX_CONFIG), so systemd shows a failed unit instead of silently running on the wrong values. The systemd units set the env vars explicitly (`Environment=CONTROLLER_CONFIG=/etc/thing-revived/controller.toml`, `Environment=TERMINAL_AGENT_CONFIG=/etc/thing-revived/terminal.toml`).

Frontend: the kiosk browser (Chromium 68, `file://`, no build step) cannot read TOML. At image build time `build/tools/toml_to_js.py` converts `frontend.toml` into a classic script `window.THING_CONFIG = {...};` (sorted keys, ASCII only, deterministic) that `index.html` loads before its other scripts. The committed `frontend/config.generated.js` is an empty stub so the app also works when served straight from the repo; the build overwrites only the copy staged for the image and never the committed stub. Never edit a generated file; edit `frontend.toml`.

## Build flow

`build/build-image.sh` validates all three files (fail early, before cargo), copies `controller.toml` and `terminal.toml` into the recipes' `files/` dirs, and generates `config.generated.js` into the webapp recipe's `files/webapp/`. To check config alone, without cloning or building anything: `CONFIG_ONLY=1 build/build-image.sh` (see `build/README.md`). Validate by hand with `python3 build/tools/toml_to_js.py --check config/*.toml`.

## Changing a value

In the repo (normal path): edit the TOML, rebuild and reflash the image.

On a running device: `/etc` is part of the read-only squashfs rootfs, so the installed files are read-only at runtime. Options:

1. Remount, edit, restart: `mount -o remount,rw /`, edit `/etc/thing-revived/<name>.toml`, then `systemctl restart controller-agent` or `systemctl restart thing-revived-terminal-agent`. Whether the edit survives a reboot depends on the rootfs layout and is unverified; it is gone after a reflash.
2. Point the agent at another file with the env var, using a runtime systemd drop-in (`systemctl edit --runtime controller-agent`, setting `Environment=CONTROLLER_CONFIG=/var/lib/thing-revived/controller.toml`). `/var` is the writable data partition, so the TOML itself persists, but a `--runtime` drop-in lives in `/run` and is gone after reboot. A persistent drop-in needs a writable `/etc/systemd/system`, i.e. the remount in option 1 or a change to the image.
3. Reflash an image built from the edited TOML.

The frontend config is baked into the image; changing it on the device means editing `config.generated.js` under `/usr/share/superbird-kiosk-default/` (same read-only caveat) or reflashing.

## Recommendation (follow-up, not implemented)

To make persistent on-device overrides possible without a remount, the agents' `utils/config.rs` could look for `/var/lib/thing-revived/<name>.toml` first and use it if present, falling back to the `/etc/thing-revived/<name>.toml` path and then to compiled-in defaults (an explicit env var still wins over both). The units need no change for that: `ExecStart` stays as is, and `/var/lib/thing-revived` already exists (tmpfiles.d, 0700, created by the wifi-config recipe). Whole-file replacement is simplest; per-key merging is optional.
