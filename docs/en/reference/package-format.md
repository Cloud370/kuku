# Package Format

Packages bundle hooks, skills, and related assets.

## Locations

- User scope: `~/.kuku/packages/<name>/`
- Project scope: `<workspace>/.kuku/packages/<name>/`

Project packages override user packages with the same name.

## Layout

```text
.kuku/packages/<name>/
├── kuku.toml
├── hooks/
├── skills/
├── .mcp.json
└── bin/
```

`kuku.toml` is the package manifest and the canonical source of truth.

## `[package]`

```toml
[package]
name = "security-guard"
version = "1.2.0"
description = "Safety net for production sessions"
homepage = "https://github.com/user/kuku-security"
repository = "https://github.com/user/kuku-security"
```

Rules:

- `name` is required, 1-64 chars, lowercase letters, digits, and hyphens
- `version` is required and must be semver

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

Or:

```toml
[[hooks]]
events = ["tool.pre_execute", "tool.post_execute"]
command = "hooks/audit-tool.sh"
```

When `events` is used, `event` must be absent.

## Hook Fields

| Field | Required | Meaning |
|---|---|---|
| `event` or `events` | yes | Trigger event name or names |
| `command` | yes | Executable path relative to package root |
| `matcher` | no | Boolean filter expression |
| `timeout_seconds` | no | Default 30, hard cap 600 |
| `chain` | no | Whether the hook receives prior hook output |
| `env` | no | Additional environment variable names to pass through |

## Implemented Hook Events

- `session.start`
- `session.end`
- `tool.pre_execute`
- `tool.post_execute`
- `model.pre_request`
- `model.post_response`

## Matcher Syntax

Operators:

- `==`
- `!=`
- `contains`
- `&&`
- `||`

Common variables:

- `event`
- `tool_name`
- `tool_call_id`
- `args.<field>`
- `status`
- `summary`
- `tier`
- `text`
- `stop_reason`

## Hook Protocol

stdin is a JSON object with minimal context, including `event` and `session_dir`.

stdout rules:

- valid JSON: treated as structured output
- non-JSON text: wrapped as `{"additional_context": "..."}`

Exit codes:

| Code | Meaning |
|---|---|
| `0` | success |
| `2` | block the operation |
| other | non-blocking hook error |

## Structured Hook Output

| Field | Meaning |
|---|---|
| `block` | Block the operation |
| `updated_args` | Replace tool arguments |
| `updated_result` | Replace tool result |
| `additional_context` | Inject extra context into the next turn |
| `permission_override` | Planned permission override field |

## MCP

`.mcp.json` uses standard MCP format. This integration is planned rather than fully documented as stable behavior.
