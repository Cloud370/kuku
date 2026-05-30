# SDK Evolution

<!-- status: partial -->

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

## Extension system (implemented)

The plugin system is implemented in the `plugin/` module and spec'd in [plugin-system.md](../extension/plugin-system.md). This section summarizes the integration points for the SDK.

Packages are discovered from `.kuku/packages/` (project) and `~/.kuku/packages/` (user). Each is a directory with a `kuku.toml` manifest and optional `hooks/` and `skills/`.

Hooks are external processes: kuku spawns them, pipes event context as stdin JSON, reads structured output from stdout. Six lifecycle events are implemented: `session.start`, `session.end`, `tool.pre_execute`, `tool.post_execute`, `model.pre_request`, `model.post_response`. Five more are planned: `turn.start`, `turn.end`, `tool.registered`, `permission.check`, `context.assembly`.

Skills inside packages use the same `SkillRegistry` as standalone skills. No migration required.

MCP servers use standard `.mcp.json` format. The `kuku-mcp` crate manages connections and implements `ExternalToolSource`.

## Implementation order

| Phase | What | Layer |
|-------|------|-------|
| 1 | Skills + SkillRegistry | SDK | ✅ |
| 2 | New UiEvent variants | SDK | ✅ |
| 3 | Wire format (`to_wire()`) | SDK | ✅ |
| 4 | `kuku-server` (HTTP API, NDJSON) | Host | ✅ |
| 5 | `apps/web` (frontend SPA) | Host | ✅ |
| 6 | Extension system + ExternalToolSource | SDK | ✅ |
| 7 | MCP extension package | Extension |
| 8 | `apps/tauri` | Host |
