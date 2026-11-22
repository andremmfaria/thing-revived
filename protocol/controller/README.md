# controller wire fixtures

One JSON file per message variant of the controller-agent WebSocket protocol (ws://127.0.0.1:8765). They are the behaviour lock for the wire format and are shared by the Rust tests (`controller/tests/wire_golden.rs`) and the frontend tests, so a change on either side that drifts from the other fails a test.

Naming: `event.<type>.json` is a message the controller sends to clients, `command.<type>.json` is a message a client sends to the controller. A second file `<kind>.<type>.<case>.json` (for example `event.wifi_status.disconnected.json`) covers an optional-field or failure shape of the same variant, mainly explicit `null` versus omitted fields.

Conventions: each file holds exactly one compact JSON message (the exact bytes the controller emits, field order included) followed by one trailing newline, which is not part of the message. Event fixtures are compared byte for byte against the serialized event. Command fixtures must parse to the expected variant; the `password` field of `wifi_connect` is deliberately a plain string here, since it is a request, and is never echoed back in any event.

Changing a fixture is a wire-format change: update the Rust types, the frontend and the docs in the same change.
