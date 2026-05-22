# SDK Evolution

<!-- status: design -->

Planned changes to the SDK public API. Each change is independent and can be implemented in any order.

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
| 1 | Skills + SkillRegistry | SDK | ✅ |
| 2 | New UiEvent variants | SDK | ✅ |
| 3 | Wire format (`to_wire()`) | SDK | ✅ |
| 4 | `kuku-server` (HTTP API, NDJSON) | Host | ✅ |
| 5 | `apps/web` (frontend SPA) | Host |
| 6 | Extension system + ExternalToolSource | SDK |
| 7 | MCP extension package | Extension |
| 8 | `apps/tauri` | Host |
