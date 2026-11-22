# Terminal protocol golden fixtures

One file per message variant of the terminal-agent WebSocket protocol (ws://127.0.0.1:8766). Each file holds exactly one text frame, byte for byte as the agent puts it on the wire, with no trailing newline (a trailing newline is tolerated by the tests and ignored).

## Conventions

- Files are named `client_<variant>.json` (frontend to agent) or `server_<variant>.json` (agent to frontend). Variants with different payload shapes get their own file, for example `server_clock_sync_ok.json` (no `age_s`) and `server_clock_sync_stale.json` (`age_s` present).
- Frames are compact JSON (no spaces) with object keys in alphabetical order. This is the byte layout the agent has always emitted (a `serde_json::Value` map is key-sorted), and the Rust tests pin it so the wire format cannot drift silently. JSON consumers must not depend on key order.
- Every frame has a snake_case `type` tag. Session ids are the integer `session` field (`index` inside `session_list`).
- `clock_sync` is unchanged on the wire, but `offset_ms` is always 0 from the real daemon: the device syncs its own system clock, and `status` reports whether `timedatectl show -p NTPSynchronized` says the clock is NTP synchronized (`ok`, `stale` or `unsynced`). The `offset_ms` values in the `clock_sync` fixtures (-42, 1500) are only there to pin the encoding of the field. `age_s` is present only when `status` is not `ok`. `code` on `exited` is `null` when the exit status is unknown.
- Output `data` is a UTF-8 string; the agent decodes PTY bytes incrementally, so multibyte characters are never split.
- Unknown or malformed client frames are ignored by the agent, not answered with an error.

## Consumers

- `daemon/tests/protocol_golden.rs`: every client fixture must decode to the typed `ClientMsg` and re-encode to identical bytes; every server fixture must equal the encoding of the matching `ServerMsg`.
- The frontend tests (see `frontend/tests/README.md`) also read these files, and a missing fixture fails their run. The controller does not use the terminal fixtures.

## Adding a variant

Add the fixture file, then add a case to the golden test. The test `every_fixture_file_is_covered` hard-codes the number of fixture files (currently `5 + 7`, client plus server) and fails when a file has no golden case, so bump that count in the same change.
