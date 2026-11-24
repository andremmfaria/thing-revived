# config/protocol/

Golden JSON fixtures for the two WebSocket protocols the frontend speaks on the device: the controller bridge (`ws://127.0.0.1:8765`) and the terminal agent (`ws://127.0.0.1:8766`). They are the shared wire contract. Each fixture is read by the Rust tests on the agent side and by the frontend tests, so a wire-format change has to touch a fixture and cannot drift silently on one side. The directory used to live at the top level as `protocol/` and now sits under `config/`.

## Layout

- `controller/` holds the controller bridge messages (buttons, knob, Wi-Fi, Bluetooth, brightness, timezone, device settings). Files are named `event.<type>[.<case>].json` (controller to client) and `command.<type>[.<case>].json` (client to controller). See [controller/README.md](controller/README.md).
- `terminal/` holds the terminal agent messages (session list, input, output, resize, control, clock sync, exit). Files are named `client_<variant>.json` (frontend to agent) and `server_<variant>.json` (agent to frontend). See [terminal/README.md](terminal/README.md).

The two directories deliberately use different naming schemes (dot-separated with `event`/`command` versus underscore-separated with `client`/`server`), and consumers match on those prefixes, so keep each directory's scheme when adding files. The byte layout also differs: controller fixtures use the field order the controller emits, terminal fixtures are compact JSON with alphabetically sorted keys. Each sub README gives the exact rules.

## Consumers

| Consumer | What it checks |
| --- | --- |
| `controller/tests/wire_golden.rs` | Reads `config/protocol/controller/` via `CARGO_MANIFEST_DIR`. Every event serializes byte for byte to its fixture, every command fixture parses to the expected variant, and a test fails if a file in the directory is not referenced. |
| `daemon/tests/protocol_golden.rs` | Reads `config/protocol/terminal/`. Client fixtures must decode and re-encode to identical bytes, server fixtures must equal the encoded `ServerMsg`, and a test fails if a fixture file has no golden case. |
| `frontend/tests/bridge-client.test.js` | Loads fixtures by prefix (`event.`, `command.`, `server_`, `client_`). Every server-side fixture must decode to a known event, and every command or client fixture must equal the frame a frontend builder produces. |
| `frontend/tests/types.test.js` | Checks that every fixture type is catalogued in the frontend type catalogue and every catalogued type has a fixture (except the client-only types it lists), and that fixture direction matches the prefix. |
| `frontend/tests/e2e/mock-server.mjs` | The e2e mock loads fixtures as `fixture('controller/event.bt_power', overrides)` straight from this directory, with no local copies, and fails if the file is missing. |

## Adding a message

1. Create the fixture in the right sub directory, with the exact bytes the agent sends or accepts, following that directory's naming and byte conventions.
2. Add or extend the Rust golden test for that side (the directory-coverage test fails until you do).
3. Make sure the frontend builder or decoder handles it and that the type is catalogued in `frontend/src/utils/types.js`, then run the frontend node tests.
4. Update the sub directory's README if the message needs explanation.

Fixtures must be valid UTF-8 JSON with one message per file and no comments. Changing a fixture is a wire-format change: update the Rust types, the frontend and the docs in the same change.
