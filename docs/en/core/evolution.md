# SDK Evolution

<!-- status: design -->

Planned changes to the SDK public API. Each change is independent and can be implemented in any order.

## InteractionRequest (planned)

Replaces `PermissionRequested` in `UiEvent`. A single mechanism for all host-agent interactions. Until this is implemented, the current API uses `PermissionRequested` and `run.decide()`.

### Current

```rust
enum UiEvent {
    PermissionRequested { request: PermissionRequest },
    // ...
}
run.decide(request_id, PermissionChoice);
```

### Planned

```rust
enum UiEvent {
    InteractionRequest { id: String, kind: InteractionKind },
    // PermissionRequested removed
}

enum InteractionKind {
    Permission { tool: String, args: String, risk: String },
    Ask { question: String, options: Vec<String> },
    Confirm { message: String },
}

enum InteractionResponse {
    Permission(PermissionChoice),
    Text(String),
    Confirm(bool),
    Cancel,
}

run.respond(id, InteractionResponse);
run.decide(id, choice);  // preserved as sugar for respond()
```

Host handles all interaction types through one code path:

```rust
match event {
    UiEvent::InteractionRequest { id, kind } => match kind {
        InteractionKind::Permission { .. } => show_permission_dialog(id),
        InteractionKind::Ask { .. } => show_ask_dialog(id),
        InteractionKind::Confirm { .. } => show_confirm_dialog(id),
    },
    _ => {}
}
```

## New UiEvent variants (planned)

Additions to the `UiEvent` enum:

| Variant | When emitted | Purpose |
|---------|-------------|---------|
| `TurnStart` | Each model turn begins | Client starts new message bubble |
| `Error { code, message }` | Provider failure | Client shows error notification |
| `ModelRequest { model, provider }` | Before provider call | Client shows model info (optional) |

```rust
enum UiEvent {
    TextDelta { text: String },
    ThinkingDelta { text: String },
    ToolCall { tool_call_id: String, tool: String, summary: String },
    ToolResult { tool_call_id: String, status: String, summary: String, structured: Option<Value> },
    InteractionRequest { id: String, kind: InteractionKind },
    TurnStart,
    Error { code: String, message: String },
    ModelRequest { model: String, provider: String },
    Done { output: RunOutput, usage: Option<ProviderUsage>, turn: u64 },
}
```

## Wire format (planned)

The SDK provides a `to_wire()` function that converts `UiEvent` to a client-friendly JSON format. Hosts call this function; they own no mapping logic.

```rust
pub fn to_wire(event: &UiEvent) -> Option<serde_json::Value>;
```

Wire events:

| Wire `type` | Source `UiEvent` | Notes |
|-------------|-----------------|-------|
| `turn_start` | `TurnStart` | |
| `text` | `TextDelta` | `{ "content": "..." }` |
| `thinking` | `ThinkingDelta` | `{ "content": "..." }` |
| `tool_start` | `ToolCall` | `{ "id", "name", "summary" }` |
| `tool_end` | `ToolResult` | `{ "id", "name", "status", "summary" }` |
| `interaction` | `InteractionRequest` | `{ "id", "kind", "payload" }` |
| `model_request` | `ModelRequest` | `{ "model", "provider" }` |
| `error` | `Error` | `{ "code", "message" }` |
| `done` | `Done` | `{ "usage" }` |

All events are streamed. The client ignores what it does not need.

## ExternalToolSource (planned)

A trait for external tool providers. The SDK dispatches tool calls to registered sources. MCP is one implementation, loaded as an extension package.

```rust
pub trait ExternalToolSource: Send + Sync {
    fn name(&self) -> &str;
    fn tools(&self) -> Vec<ToolDefinition>;
    fn call(&self, tool: &str, args: serde_json::Value) -> Result<ToolResultEnvelope>;
}
```

The tool registry merges built-in tools with external tools. Skills inject instructions, not tools. The permission gate applies uniformly.

```text
ToolRegistry {
    builtins: [find_files, read_file, ...],
    external: [mcp_github.search, ...],     // from ExternalToolSource (planned)
}
```

MCP is not implemented in the SDK. A future `mcp-client` extension package implements `ExternalToolSource` for MCP servers.

## Extension system (planned)

A package loader that discovers and loads extensions from `.kuku/packages/`.

```text
.kuku/packages/tdd-suite/
├── kuku.toml           # manifest
├── skills/             # skills
├── mcp-config.json     # MCP server config
└── tools/              # custom tools
```

```toml
# kuku.toml
[package]
name = "tdd-suite"
version = "0.1.0"

[skills]
tdd = "skills/tdd.md"

[mcp]
servers = ["mcp-config.json"]
```

The extension system is SDK core infrastructure. Skills are native to the SDK. MCP, hooks, and custom tools are extension types loaded through this system.

## Implementation order

| Phase | What | Layer |
|-------|------|-------|
| 1 | Skills + SkillRegistry | SDK |
| 2 | InteractionRequest + new UiEvent variants | SDK |
| 3 | Wire format (`to_wire()`) | SDK |
| 4 | `apps/server` (HTTP API, NDJSON) | Host |
| 5 | `apps/web` (frontend SPA) | Host |
| 6 | Extension system + ExternalToolSource | SDK |
| 7 | MCP extension package | Extension |
| 8 | `apps/tauri` | Host |
