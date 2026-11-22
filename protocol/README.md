# protocol/

Golden JSON fixtures for the WebSocket messages exchanged between the on-device agents and the frontend. They are the shared contract: the Rust tests in `controller/` and `daemon/` and the frontend node and e2e tests in `frontend/tests/` all read the same files, so a wire-format change has to touch a fixture and cannot drift silently on one side.

## Layout

- `protocol/controller/` holds the controller-agent messages (buttons, knob, and so on). See its own README.
- `protocol/terminal/` holds the terminal-agent messages (session, input, output, resize, and so on). See its own README.

## Naming

One file per message variant, named `<direction>-<type>[-<variant>].json`.

- `<direction>` is `server` for agent to frontend and `client` for frontend to agent.
- `<type>` is the message's `type` field (kebab-case if it has more than one word).
- `<variant>` is optional and distinguishes shapes of the same type (for example an optional field present or absent).

Examples: `server-button.json`, `client-resize.json`, `server-button-long-press.json`.

## Adding a message

1. Create the fixture in the right sub directory, with the exact JSON the agent sends or accepts, using the naming above.
2. Add or extend a Rust test that parses the fixture, and for server messages also serialises the Rust value and compares it to the fixture as JSON values (not as text, so key order is irrelevant).
3. Add or extend a frontend node test that feeds the fixture through the frontend's message handling.
4. Update the sub directory's README if the message needs explanation.

Keep fixtures minimal and valid: no comments, no trailing commas, UTF-8, one message per file.
