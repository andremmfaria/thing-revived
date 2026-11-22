# controller wire fixtures

One JSON file per message variant of the controller WebSocket bridge protocol (default `ws://127.0.0.1:8765`, set by `bind` in `config/controller.toml`). They are the behaviour lock for the wire format, shared by the controller Rust tests and the frontend tests, so a change on either side that drifts from the other fails a test. The Rust types are in `controller/src/protocol/` (`event.rs` for `BridgeEvent`, `command.rs` for `ClientCommand`). Every message is a JSON object with a `type` tag. The sibling directory `config/protocol/terminal/` holds the terminal agent fixtures, and `config/protocol/README.md` covers the shared procedure.

## Message families

Each family has commands from the client and events from the controller. The file name is the `type` value.

| Family | Commands | Events |
| --- | --- | --- |
| Input | none | `button`, `knob` |
| Bluetooth | `bt_power_get`, `bt_set_power`, `bt_scan_start`, `bt_scan_stop`, `bt_pair`, `bt_pairing_confirm`, `bt_paired_list_get`, `bt_connect`, `bt_disconnect`, `bt_forget` | `bt_power`, `bt_scan_started`, `bt_scan_stopped`, `bt_device_found`, `bt_pairing_confirm_request`, `bt_pairing_passkey_display`, `bt_pair_result`, `bt_paired_list`, `bt_error` |
| Settings | `brightness_get`, `brightness_set`, `timezone_get`, `timezone_set`, `timezone_list_get` | `brightness`, `timezone`, `timezone_list`, `settings_error` |
| Wi-Fi | `wifi_get`, `wifi_scan`, `wifi_connect`, `wifi_connect_saved`, `wifi_disconnect`, `wifi_saved_list_get`, `wifi_forget` | `wifi_available`, `wifi_status`, `wifi_scan_started`, `wifi_scan_results`, `wifi_connecting`, `wifi_connect_result`, `wifi_saved_list`, `wifi_error` |
| Users (first-boot password) | `user_bootstrap_get`, `user_bootstrap_reveal`, `user_bootstrap_ack` | `user_bootstrap`, `user_bootstrap_secret`, `user_error` |

The Users family is Phase 1 of `docs/research/user-management-plan.md` (section 8): the first-boot admin password reveal. `user_bootstrap_get` asks for the state (reply `user_bootstrap`), `user_bootstrap_reveal` asks for the one-time password (reply `user_bootstrap_secret`, or `user_error` when no knob push was seen in the press window), and `user_bootstrap_ack` carries the last four characters as proof (reply `user_bootstrap`, or `user_error` with code `bad_ack`). The controller sends every `user_*` reply to the requesting connection only and logs it by type only. `user_error` carries `context` (the request type), `code` (a stable code such as `bad_ack`, `not_unlocked`, `storage_unavailable`, `internal`) and `message` (display text). Phases 2a and 2b add more `user_*`, `group_*` and `ssh_key_*` messages later.

## Naming

`event.<type>.json` is a message the controller sends to clients, and `command.<type>.json` is a message a client sends to the controller. A third segment, `<kind>.<type>.<case>.json`, covers an optional-field or failure shape of the same variant, mostly an explicit `null` versus a populated value. Examples: `event.wifi_status.disconnected.json`, `event.bt_device_found.unnamed.json`, `event.bt_pair_result.failed.json`, `event.wifi_available.none.json`, `command.wifi_connect.open.json`, `event.user_bootstrap.none.json` (nothing pending, `expires_in_secs` is `null`).

## Conventions

Each file holds exactly one compact JSON message, the exact bytes the controller emits with field order included, followed by one trailing newline that is not part of the message. Unset optional fields are serialized as explicit `null`, not omitted. Command fixtures only have to parse to the expected variant. The `password` field of `wifi_connect` is a plain string in the fixture because it is a request, and the controller redacts it in its own debug output and never echoes it in any event. `command.wifi_connect.open.json` is the open network form with an empty password.

Secrets: the fixtures use synthetic values only. The password fields (`password` of `wifi_connect`, `password` of `user_bootstrap_secret`, `last4` of `user_bootstrap_ack`) are `Secret` in Rust, whose `Debug` and `Display` output is always `<redacted>`. `Secret` serializes the real value, so `user_bootstrap_secret` is the only event allowed to carry a password; `wire_golden.rs` asserts that, and that no event shows one in its `Debug` output.

## Which tests read them

- `controller/tests/wire_golden.rs` serializes every `BridgeEvent` and compares it byte for byte with its fixture, parses every command fixture and checks the variant, checks the `wifi_connect` password handling and the `user_bootstrap_ack` proof, checks that only `user_bootstrap_secret` serializes a password and that no event leaks one through `Debug`, and fails if the directory contains a `.json` file it does not reference, so a new fixture needs a matching entry there.
- `frontend/tests/bridge-client.test.js` and `frontend/tests/types.test.js` read the `event.` and `command.` files directly, and the e2e harness (`frontend/tests/e2e/mock-server.mjs` and `menu-regression.mjs`) replays them to the kiosk app. See `frontend/tests/README.md`.

The fixtures are test data only and are not installed on the device.

Changing a fixture is a wire-format change: update the Rust types, the frontend and the docs in the same change.
