# Session files and the Rust SDK

## Inspectable Session journals

Every run writes a durable journal on disk. Use `kuku show SESSION_ID` to read the latest answer and `kuku events SESSION_ID` to inspect recorded inputs, model requests, tool calls, and results. Whenever an Agent edits a file or runs a command, you can cross-check the journal against your workspace directly. [Manage Sessions](../guides/manage-sessions.md)

## Resumable, multi-Agent Sessions

Reopen any Session later and pick up with each Agent's context, pending requests, and saved tool outputs intact. Disconnecting a TUI, CLI, or browser client leaves accepted work running in the service. Multiple Agents can share a Session and work on the same workspace concurrently, each with its own address, inbox, and context. [File-native execution](file-native-model.md) · [Run the server](../guides/run-the-server.md)

## Embeddable Rust runtime

Embed the Rust SDK directly in your own application to run Agents and persist Sessions in-process, with no daemon or CLI subprocess required. The terminal, scripts, and browser all drive the exact same runtime through the HTTP server. [Rust SDK](../reference/sdk.md) · [Server API](../reference/server-api.md)
