# Package format

Packages bundle hooks, Skills, and related assets.

## Locations and layout

- User scope: `~/.kuku/packages/<name>/`
- Project scope: `<workspace>/.kuku/packages/<name>/` (overrides user packages with the same name)

```text
.kuku/packages/<name>/
├── kuku.toml
├── hooks/
├── skills/
├── .mcp.json
└── bin/
```

`kuku.toml` is the manifest and source of truth.

## `[package]`

```toml
[package]
name = "security-guard"
version = "1.2.0"
description = "Safety net for production sessions"
homepage = "https://github.com/user/kuku-security"
repository = "https://github.com/user/kuku-security"
```

- `name` (required): 1–64 chars, lowercase letters, digits, hyphens.
- `version` (required): valid semver.

## `[[hooks]]`

```toml
[[hooks]]
event = "tool.pre_execute"
command = "hooks/pre-check.sh"
matcher = 'tool_name == "run_command"'
timeout_seconds = 30
chain = false
env = ["MY_TOKEN"]
```

Or match multiple events (omit `event` when `events` is used):

```toml
[[hooks]]
events = ["tool.pre_execute", "tool.post_execute"]
command = "hooks/audit-tool.sh"
```

| Field | Required | Meaning |
|---|---|---|
| `event` or `events` | yes | Trigger event name(s) |
| `command` | yes | Executable path relative to package root |
| `matcher` | no | Boolean filter expression |
| `timeout_seconds` | no | Default `30`, hard cap `600` |
| `chain` | no | Pass prior hook output to this hook |
| `env` | no | Extra environment variable names to forward |

**Implemented events**: `session.start`, `session.end`, `tool.pre_execute`, `tool.post_execute`, `model.pre_request`, `model.post_response`.

Matchers use the operators `==`, `!=`, `contains`, `&&`, and `||`. Common variables include `event`, `tool_name`, `tool_call_id`, `args.<field>`, `status`, `summary`, `text`, and `stop_reason`.

## Hook protocol

- **stdin**: JSON object with minimal context (including `event` and `session_dir`).
- **stdout**: valid JSON is parsed as structured output; plain text is wrapped as `{"additional_context": "..."}`.
- **Exit codes**: `0` = success, `2` = block the operation, any other code = non-blocking hook error.

Structured stdout fields:

| Field | Meaning |
|---|---|
| `block` | Block the operation |
| `updated_args` | Replace tool arguments |
| `updated_result` | Replace tool result |
| `additional_context` | Inject extra context into the next Turn |
| `permission_override` | Planned permission override field |

## MCP

`.mcp.json` follows standard MCP format; runtime MCP integration is still planned rather than stable behavior.
