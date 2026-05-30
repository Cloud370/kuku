# Plugin System

<!-- status: design -->

A package loader that discovers and activates extensions from `.kuku/packages/`. Packages bundle hooks, skills, and MCP server configurations. The runtime executes hooks as external processes — language-agnostic, auditable, and isolated.

## Philosophy

**External process.** Every hook is an executable. kuku spawns it, pipes context to stdin, reads results from stdout, and checks the exit code. No Wasm, no dynamic linking, no language binding. Authors write hooks in any language.

**Minimal input.** Hook stdin carries a lightweight pointer — event type, session directory, and event-specific parameters. Hooks that need deeper context read `events.jsonl`, `memory.md`, or other session files directly from disk. This avoids serializing context twice and keeps the protocol thin.

**Trust is the user's responsibility.** kuku does not verify hook identity (hash-based trust, workspace trust, or allowlists). If you install a package, you accept its cost. This is distinct from runtime safety: the permission gate and hard guard still enforce tool-level restrictions regardless of which hooks are loaded. Hooks can observe and modify events, but they cannot bypass the permission gate.

**Framework provides mechanism, never policy.** Timeout defaults exist but authors set their own. Output gets truncated at 100 KB but the overflow is written to disk, not discarded. The framework sets a hard ceiling (600 s, 100 KB) and steps out of the way.

## Package Layout

```
.kuku/packages/<name>/
├── kuku.toml          # manifest — the single source of truth
├── hooks/             # executable hook scripts
├── skills/            # Agent Skills spec — auto-discovered
├── .mcp.json          # standard MCP configuration (optional)
└── bin/               # auxiliary binaries and scripts
```

### Loading tiers

Three tiers, resolved highest-priority-first:

| Tier | Location | Scope |
|------|----------|-------|
| Built-in | `<kuku-bin>/packages/` | Shipped with the kuku binary |
| User | `~/.kuku/packages/` | All projects for one user |
| Project | `.kuku/packages/` | One project, committed to git |

Project overrides user. User overrides built-in.

Skills are auto-discovered from `skills/` directories within loaded packages. No explicit manifest declaration required — same scanning logic as standalone `.kuku/skills/`.

## Lifecycle Events

Eleven events cover six boundaries of the agent loop: session, turn, tool, model, permission, and context.

| # | Event | Triggers when | Block | Modify input | Modify output | Force continue |
|---|-------|--------------|:-----:|:------------:|:-------------:|:--------------:|
| 1 | `session.start` | Session created, before first turn | ✓ | — | ✓ | — |
| 2 | `session.end` | Session about to close | — | — | ✓ | — |
| 3 | `turn.start` | Each turn begins | — | — | ✓ | — |
| 4 | `turn.end` | Each turn completes | — | — | ✓ | — |
| 5 | `tool.registered` | Tool added to registry | ✓ | ✓ | — | — |
| 6 | `tool.pre_execute` | Before tool execution (after permission gate) | ✓ | ✓ | — | — |
| 7 | `tool.post_execute` | After tool execution completes | — | — | ✓ | — |
| 8 | `model.pre_request` | Context assembled, before provider call | — | ✓ | — | — |
| 9 | `model.post_response` | Provider returns, before event persistence | — | — | ✓ | ✓ |
| 10 | `permission.check` | Permission gate decision point | — | ✓ | — | — |
| 11 | `context.assembly` | Context rebuild complete | — | ✓ | — | — |

**Block** (exit code 2): prevents the operation from proceeding.
**Modify input**: alters parameters, messages, or tool schemas before the operation.
**Modify output**: alters results, appends additional context, or overrides decisions.
**Force continue** (`model.post_response` only): exit code 2 injects `additional_context` as a user message, resuming the agent loop.

## `kuku.toml` Schema

```toml
[package]
name = "security-guard"              # required: 1-64 chars, lowercase + digits + hyphens
version = "1.2.0"                    # required: semver
description = "Safety net for production sessions"
homepage = "https://github.com/user/kuku-security"
repository = "https://github.com/user/kuku-security"

[[hooks]]
event = "tool.pre_execute"           # required: event name
command = "hooks/pre-check.sh"       # command or url (one required)
matcher = 'tool_name == "run_command"'  # optional: filter expression
timeout_seconds = 30                 # optional: default 30 s, hard cap 600 s
chain = false                        # optional: receive prior hook's output

[[hooks]]
event = "session.start"
command = "hooks/setup-env.sh"
```

### Multiple events per hook

A single hook declaration may listen on multiple events:

```toml
[[hooks]]
events = ["tool.pre_execute", "tool.post_execute"]
command = "hooks/audit-tool.sh"
```

When `events` is used, `event` must be absent. The hook receives the triggering event name in its stdin.

### Matcher expression syntax

```
# Operators
==  !=          equality / inequality
&&  ||          logical and / or
contains         string containment

# Available variables (event-dependent)
tool_name        tool.* events
args.<field>     tool call arguments
source           session.start, session.end
```

If `matcher` is absent, the hook fires on every occurrence of the event.

## Hook Execution Protocol

Hooks are spawned once per event invocation — a new process for each trigger. This keeps the protocol stateless: the stdin JSON is the complete input, stdout is the complete output, and the process exits when done. Hooks that need persistent state (caches, connection pools) can use a background HTTP server and the `url` hook variant.

### stdin

A JSON object carrying minimal context:

```json
{
  "event": "tool.pre_execute",
  "session_dir": "/home/user/.kuku/p/project/sessions/abc123/",
  "tool_name": "run_command",
  "tool_args": {"command": "git push"},
  "tool_call_id": "tool_01"
}
```

`session_dir` is the anchor — hooks read `events.jsonl`, `memory.md`, and other session files directly from disk when they need richer context. The protocol does not duplicate what is already on disk.

Event-specific fields vary by event type. `tool.*` events carry `tool_name`, `tool_args`, `tool_call_id`. `session.*` events carry `source`. `model.*` events carry a summary of the request or response. Full schema will be defined in the implementation crate.

### stdout — hybrid mode

- If stdout parses as valid JSON → processed as structured output
- If stdout is not JSON → automatically wrapped as `{"additional_context": "<stdout text>"}`

### Structured output fields

| Field | Type | Applies to | Effect |
|-------|------|-----------|--------|
| `block` | bool | `tool.pre_execute`, `session.start` | Block the operation. Equivalent to exit code 2. |
| `updated_args` | object | `tool.pre_execute` | Replace tool arguments |
| `updated_result` | object | `tool.post_execute` | Replace tool result |
| `additional_context` | string | all | Injected into the next model turn |
| `permission_override` | `"allow"` or `"deny"` | `permission.check` | Override the permission decision |

### exit code

| Code | Meaning |
|------|---------|
| 0 | Success. stdout processed normally. |
| 2 | Block the operation. stderr recorded as the block reason. |
| Other | Non-blocking error. stderr logged as a warning; operation proceeds. |

### Environment

Hook processes receive only a filtered subset of the parent environment. The following variables are explicitly set:

| Variable | Value |
|----------|-------|
| `KUKU_SESSION_DIR` | Absolute path to the current session directory |
| `KUKU_WORKSPACE` | Absolute path to the workspace root |
| `KUKU_PACKAGE_DIR` | Absolute path to the package root directory |
| `PATH` | Inherited from parent |

All other variables (including `ANTHROPIC_API_KEY`, `GITHUB_TOKEN`, and other secrets) are **not** passed to hook processes. Hooks that need credentials must source them independently.

## Execution Model

### Serial by default

Hooks for the same event execute serially in the order they appear in `kuku.toml`. Serial execution is deterministic and easy to debug.

### Chaining

When `chain = true`, a hook receives the **merged output of all previous hooks** in its stdin under the key `_chain`. When `chain = false` (default), every hook sees the original, unmodified event input. Chain mode enables multi-stage pipelines (one hook validates, the next transforms) within a single event.

### Timeout

Default 30 seconds per hook. Authors can set any value via `timeout_seconds`. The hard cap is 600 seconds — values above this are clamped. On timeout, the hook process receives SIGTERM; if it does not exit within 2 seconds, SIGKILL. The hook is treated as a non-blocking error (stderr logged, operation proceeds).

### Output overflow

stdout exceeding 100 KB (approx. 25k tokens) is truncated. The full output is written to the session directory:

```
<sessions>/<id>/hook_overflow/<hook_index>_<timestamp>.out
```

The truncated result carries the file path so the model can read it if needed.

## MCP Integration

MCP servers are declared in standard `.mcp.json` format at the package root. kuku discovers these files during package loading and delegates connection management to the `kuku-mcp` crate.

```json
{
  "mcpServers": {
    "github": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-github"],
      "env": {
        "GITHUB_TOKEN": "${GITHUB_TOKEN}"
      }
    }
  }
}
```

`.mcp.json` follows the MCP specification format — no kuku-specific schema. Configurations are portable across Claude-Code, codex, and other MCP-compatible hosts.

MCP tools enter the tool registry through the `ExternalToolSource` trait. They are subject to the same permission gate, hook execution, and naming conventions (`mcp_<server>_<tool>`) as all other tools. MCP server lifecycle (spawn, connect, reconnect, shutdown) is handled by `kuku-mcp`, not by the hook system.

## Relationship to Skills

Skills are a subset of the package format. A bare skill in `.kuku/skills/tdd/SKILL.md` is the lightweight path. The same skill inside a package at `.kuku/packages/tdd-suite/skills/tdd/SKILL.md` gains the ability to ship hooks and MCP servers alongside it. Both paths use the same `SkillDefinition` and `SkillRegistry`. No migration required.

When a standalone skill and a package-bundled skill share the same name, the package-bundled version takes precedence. This follows the standard loading priority: project > user > built-in.

## Forward Compatibility

The `kuku.toml` format and event list are versioned implicitly through the `[package] version` field. Future additions to the event list, matcher syntax, or output fields will be additive — existing hooks continue to work unchanged. Unknown fields in `kuku.toml` are ignored (not errors), enabling forward-compatible manifests.
