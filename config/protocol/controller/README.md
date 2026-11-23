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
| Users | `user_bootstrap_get`, `user_unlock`, `user_lock_session`, `user_list_get`, `user_add`, `user_remove`, `user_set_locked`, `user_set_password`, `user_group_add`, `user_group_remove`, `group_list_get`, `group_add`, `group_remove`, `ssh_key_list_get`, `ssh_key_add`, `ssh_key_remove` | `user_bootstrap`, `user_bootstrap_secret`, `user_error`, `user_session`, `user_list`, `user_result`, `user_secret`, `group_list`, `ssh_key_list` |

The Users family is defined by `docs/research/user-management-plan.md` section 8 as amended by `docs/research/user-management-plan-delta-password-auth.md` section 5.6. First boot: the frontend sends `user_bootstrap_get` once it is ready (reply `user_bootstrap`, and `user_bootstrap_secret` too while the phase is `showing`). `user_bootstrap` carries `phase` (`none`, `prompt`, `generating`, `showing`, `done`), `username` and `storage_ok`, is broadcast on every phase change and carries no timer or counter. Generation and acknowledgement are the physical M button only, so there is no reveal or ack command. `user_bootstrap_secret` carries the password and no expiry; the controller sends it to each ready connection when `showing` starts. Account menu (Phase 2a) and groups and SSH keys (Phase 2b): `user_unlock` opens an admin session (`user_session`), the mutating commands answer with `user_result` (or `user_error`), a generated password comes back once as `user_secret`, and `user_list` (with `key_count`), `group_list` and `ssh_key_list` answer the list requests. No key is baked into the image, so `ssh_key_list` has no `baked` field. `confirm` echoes the target (user name, group name or key fingerprint) for destructive requests; `user_group_add` needs it for `sudo`. An empty `password` in `user_add` and `user_set_password` means: generate one. The controller sends every `user_*`, `group_*` and `ssh_key_*` reply to the requesting connection only and logs it by type only. `user_error` carries `context` (the request type), `code` (one of `generate_failed`, `ack_failed`, `not_unlocked`, `bad_password`, `rate_limited`, `invalid_name`, `invalid_password`, `invalid_key`, `exists`, `leftover_state`, `not_found`, `system_account`, `protected_account`, `last_admin`, `group_not_assignable`, `too_many`, `confirm_required`, `no_password`, `sessions_remain`, `user_busy`, `storage_unavailable`, `internal`) and `message` (display text).

## Naming

`event.<type>.json` is a message the controller sends to clients, and `command.<type>.json` is a message a client sends to the controller. A third segment, `<kind>.<type>.<case>.json`, covers an optional-field or failure shape of the same variant, mostly an explicit `null` versus a populated value. Examples: `event.wifi_status.disconnected.json`, `event.bt_device_found.unnamed.json`, `event.bt_pair_result.failed.json`, `event.wifi_available.none.json`, `command.wifi_connect.open.json`, `event.user_bootstrap.showing.json` (one file per `phase`), `event.user_session.locked.json` (`expires_in_secs` and `username` are `null`).

## Conventions

Each file holds exactly one compact JSON message, the exact bytes the controller emits with field order included, followed by one trailing newline that is not part of the message. Unset optional fields are serialized as explicit `null`, not omitted. Command fixtures only have to parse to the expected variant. The `password` field of `wifi_connect` is a plain string in the fixture because it is a request, and the controller redacts it in its own debug output and never echoes it in any event. `command.wifi_connect.open.json` is the open network form with an empty password.

Secrets: the fixtures use synthetic values only. The password fields (`password` of `wifi_connect`, `password` of `user_bootstrap_secret` and `user_secret`, the password fields of the `user_*` commands) are `Secret` in Rust, whose `Debug` and `Display` output is always `<redacted>`. `Secret` serializes the real value, so `user_bootstrap_secret` and `user_secret` are the only events allowed to carry a password; `wire_golden.rs` asserts that, and that no event shows one in its `Debug` output.

## Which tests read them

- `controller/tests/wire_golden.rs` serializes every `BridgeEvent` and compares it byte for byte with its fixture, parses every command fixture and checks the variant, checks the `wifi_connect` and user command password handling, checks that the removed commands and error codes stay gone, checks that only the two secret events serialize a password and that no event leaks one through `Debug`, and fails if the directory contains a `.json` file it does not reference, so a new fixture needs a matching entry there.
- `frontend/tests/bridge-client.test.js` and `frontend/tests/types.test.js` read the `event.` and `command.` files directly, and the e2e harness (`frontend/tests/e2e/mock-server.mjs` and `menu-regression.mjs`) replays them to the kiosk app. See `frontend/tests/README.md`.

The fixtures are test data only and are not installed on the device.

Changing a fixture is a wire-format change: update the Rust types, the frontend and the docs in the same change.
