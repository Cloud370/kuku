# Glossary

Canonical names for kuku concepts. When writing or editing docs, use these names consistently. When a concept changes, `grep` its canonical name to find every document that needs updating.

## Session & Events

| Canonical Name | Definition |
|---------------|------------|
| `workspace` | The project root directory. All file tool paths are relative to workspace. Determines where project instructions, memory, and policy are found. |
| `kuku_home` | `$KUKU_HOME` directory. Stores project-scoped sessions (`p/<workspace-path>/sessions/`), config (`config.toml`), and user-level memory. |
| `session` | One agent execution lifecycle. A session directory under `$KUKU_HOME/p/<workspace>/sessions/<id>/` contains `events.jsonl` and derived views. Child sessions live alongside under `sessions/child_<parent>_<N>/`. |
| `events.jsonl` | Append-only JSONL file. Every fact the runtime observes (model requests, tool calls, permission decisions) is a line. The ground truth for replay and inspection. |
| `turn` | One request→response→tools cycle within a session. Starts with `turn.start`, ends with `turn.end`. |
| `StoredEvent` | A single line in `events.jsonl`: `{id, ts, payload}`. `EventPayload` is the tagged union of all known event types. |
| `user.input` | Event recording the human prompt that started the current turn. |
| `model.request` | Event recording a provider request: resolved provider/model, params, provenance (tool registry, subagent registry, instruction/memory hashes), and rendered context snapshot. |
| `model.response` | Event recording a completed provider response: stop reason, usage. Followed by zero or more `tool.call` events; together they form a response group. |
| `model.error` | Diagnostic event for provider failures (auth, rate limit, network, overflow). Does not become a model message. |
| `tool.call` | Event recording a single tool use requested by the model within a response group. |
| `tool.result` | Event recording the outcome of a `tool.call`: status (`ok`, `error`, `blocked`, `cancelled`), summary, model_content, truncated, structured. |
| `turn.start` | Event marking the beginning of a turn. |
| `turn.end` | Event marking the end of a turn. |
| `response group` | A `model.response` and its immediately following `tool.call[]` events, treated as one assistant message during context rebuild. |
| `SessionSummary` | Metadata struct for a listed session: session_id, workspace, title, created_at, turn_count, status, mtime, size. Extracted from disk without full event replay. |
| `SessionStatus` | Enum classifying a session's state: `Active` (lock + live PID), `Done` (last event is `turn.end`), `Interrupted` (otherwise). |

## Context

| Canonical Name | Definition |
|---------------|------------|
| `context rebuild` | Reconstruct provider-neutral messages from `events.jsonl` + file-backed sources (instructions, memory). Stateless per request. |
| `context budget` | Available context window headroom, classified into tiers (`tight` / `normal` / `roomy`). Determines which dynamic content (notices, catalogs) is included. |
| `request provenance` | Metadata attached to every `model.request` event: workspace, instruction/memory/prompt hashes, tool registry snapshot, subagent registry snapshot, history range. The anchor for request inspection. |
| `canonical message` | Provider-neutral message with typed `MessageBlock`s (text, tool_use, tool_result). Converted to provider-native format by adapters. |
| `system prompt` | Stable runtime contract: identity, hard rules, working style. Does not contain workspace, date, memory, or task-specific content. |
| `project_context` | Behavior framework rendered into messages[3]: project instructions, execution context (workspace/platform/date), model tiers. |
| `runtime_context` | Dynamic catalogs and notices rendered into the current user turn: agent catalog, drift notices. Separated from project_context for cache stability. |
| `tool_guidance` | Tool usage guidance, rendered as messages[0] (first prelude message). Shared across all users and projects. |
| `global_memory` | User-level memory rendered as messages[1]. Same user, same memory → cache hit across projects. |
| `project_memory` | Project-level memory rendered as messages[2]. Same project, same memory → cache hit within project. |
| `project instructions` | `AGENTS.md` / `CLAUDE.md` files loaded as the primary behavior source for a session. |
| `memory` | Long-lived background context stored in `memory.md` files. Two layers: global (user-level) and project (workspace-level). |
| `system notice` | Runtime-injected `<kuku_system_notice>` block. Signals tool set changes, permission posture changes, or file-backed context drift. |
| `context drift` | Signal that file-backed context (memory, instructions) has changed since the session started. Does not re-inject content; only notifies the model that a change occurred. |

## Provider

| Canonical Name | Definition |
|---------------|------------|
| `provider` | Model API adapter. Converts canonical messages + tools into provider-native requests, and provider responses back into normalized events. |
| `provider format` | One of `anthropic`, `openai-chat`, `openai-responses`. Each defines how requests are built and responses parsed. |
| `streaming` | Real-time event flow: provider SSE stream → adapter normalizes → `UiEvent` yields to host. Streaming delta is not persisted until the response completes. |

## SDK vs Host

| Canonical Name | Definition |
|---------------|------------|
| `SDK` | Runtime semantics and facts: sessions, events, context rebuild, provider adapters, tool dispatch, permission decisions, and persistence. |
| `host` | Presentation and interaction layer: CLI, TUI, WebUI, command routing, layout, input, and output rendering. |

## Tool

| Canonical Name | Definition |
|---------------|------------|
| `tool` | An agent capability with a name, JSON schema, and execution handler. Built-in tools: `find_files`, `read_file`, `search_text`, `edit_file`, `write_file`, `remember_memory`, `forget_memory`, `run_command`, `agent`. |
| `tool registry` | Ordered, stable list of tool definitions for a request. Built-in tools are fixed-order; `agent` tool is conditionally appended. |
| `tool result envelope` | Unified return type for all tool executions: `status`, `summary`, `model_content`, `truncated`, `structured`. |
| `read snapshot` | File identity recorded on successful `read_file`: canonical path, content hash, event id, line range. Enables read caching and write/edit precondition checks. |
| `display summary` | Human-readable one-liner derived from tool args and result. Used by CLI/TUI to show what a tool did without exposing raw output. |
| `ExecSlot` | Per-tool runtime unit in `Run`. Holds an independent `tokio::sync::Notify` cancel token and a child-permission channel map. All tools (including agent) run in their own slot concurrently. |
| `ToolKind` | Classifies a tool execution: `Simple` (builtin tools), `Agent { child_session_id }` (subagent child session), `Command { pid }` (run_command). Used by `ExecSlot` and `ToolStart` events. |
| `ToolEvent` | Runtime event produced inside an `ExecSlot`: `TextDelta`, `ThinkingDelta`, `ToolStart`, `ToolOutput`, `ToolEnd`, `Stdout`, `Stderr`, `PermissionRequested`, `Error`. Forwarded to `Run::next()` as `UiEvent`. |
| `cancel_tool` | `Run::cancel_tool(tool_call_id)` method. Cancels a single running tool by notifying its `ExecSlot` cancel token. Returns `true` if the slot was found and cancelled. |

## Permission

| Canonical Name | Definition |
|---------------|------------|
| `permission gate` | Runtime decision point before every tool execution. Evaluates hard guard, policy, session grants, and defaults. |
| `permission.request` | Event recording a tool awaiting authorization. Written before every gated tool. |
| `permission.decision` | Event recording the gate outcome: `allow`, `deny`, or deferred to host. |
| `hard guard` | Non-bypassable safety rules. Denies writes to `.git/`, secrets, system paths. Overrides all other permission sources. |
| `policy.md` | Project-level permission rules: allow and deny patterns for specific tools and paths. |

## Config

| Canonical Name | Definition |
|---------------|------------|
| `tier` | Model capability preset: `strong` / `balanced` / `light`. Each tier maps to a `[model.X]` section in config. |
| `think level` | Thinking/reasoning setting: `off` / `low` / `medium` / `high`. Controls provider-level reasoning effort. `high` maps to Anthropic `effort:"max"`. |
| `config.toml` | Typed configuration file at `$KUKU_HOME/config.toml`. Defines providers and model tiers. Loaded once per session. |
| `provider config` | A `[provider.X]` section declaring format, base_url, and api_key for one model provider. |

## Subagent

| Canonical Name | Definition |
|---------------|------------|
| `subagent` | A tool-backed child session. The main agent dispatches via the stable `agent` tool; runtime spawns an isolated child session under `sessions/child_<parent>_<N>/`. |
| `SubagentDefinition` | Internal representation of a subagent: name, description, instructions, tier, tool_profile, tools, max_turns, source, hash. |
| `tool_profile` | Allowed tool preset for a subagent: `none` / `read` / `read_write`. Mapped to a concrete tool allowlist at spawn time. |
| `agent tool` | The single stable tool (`name: "agent"`) for dispatching subagents. Schema is fixed; available agents are declared in the catalog. |
| `child session` | Isolated session created under `sessions/child_<parent>_<N>/`. Has its own `events.jsonl`, constrained tool registry, and capped permissions. |
| `depth guard` | Hard limit on subagent nesting: maximum 2 levels (parent → child → grandchild). Checked via `child_session_count` in `PendingRun`; a tool call that would exceed depth is blocked with status `"blocked"` and a descriptive summary. |
| `agent catalog` | Short XML block listing available subagents (name, description, tier, tool_profile, hash). Injected into `runtime_context`. Does not include full instructions. |
| `subagent registry` | Loaded set of `SubagentDefinition`s from builtins + discovery-based scanning of configured and auto-detected directories. Content-hashed for drift detection. |

## Extension

| Canonical Name | Definition |
|---------------|------------|
| `package` | (planned) Extension container: manifest, capabilities, resources. Not part of core runtime. |
| `hook` | (planned) Extension point allowing packages to intercept runtime events. |
| `MCP` | (planned) Model Context Protocol integration. External tools and resources exposed through the MCP protocol, gated through the standard tool registry and permission model. |
| `skill` | A packaged capability (instructions, scripts, references) that extends the current session. Follows the Agent Skills specification. Discovered through a catalog, loaded on demand via `use_skill`. See [skills.md](core/skills.md). |
| `SkillDefinition` | Internal representation of a skill: name, description, instructions, source, hash, source_path, allowed_tools, disallowed_tools, max_turns, model, license, compatibility, metadata. |
| `SkillRegistry` | Loaded set of `SkillDefinition`s from user and project directories via pattern-based discovery scanning. Content-hashed for drift detection. Metadata injected into `runtime_context` at startup. |
| `use_skill` | Built-in tool that loads a skill's full `SKILL.md` body into the current session on demand. Reads from disk for hot-reload support. |
| `skill catalog` | XML block listing available skills (name, description, source, hash). Injected into `runtime_context` after the agent catalog. Does not include full instructions. |
| `plugin` | (planned) Synonym for `package`. A plugin is a package that may include hooks, skills, tools, or MCP servers. |
| `host overlay` | Host-specific prompt layer (CLI, TUI, WebUI). Complements but does not redefine the system prompt. |

## Public API

| Canonical Name | Definition |
|---------------|------------|
| `query()` | Free function `kuku::query(prompt)` that returns a `Query` builder. The primary SDK entry point. |
| `Query` | Typed builder returned by `kuku::query(prompt)`. Configure workspace, tier, config, session, and subagents via chained methods. Call `.run()` or `.start()`. |
| `Run` | Handle to an active query. Stream `UiEvent` via `.next()`, respond to permissions via `.decide()`, read `.session_id()`. |
| `UiEvent` | Event streamed from SDK to host. Variants: `TextDelta`, `ThinkingDelta`, `ToolStart`, `ToolOutput`, `ToolEnd`, `PermissionRequested`, `Cancelled`, `TurnStart`, `Error`, `ModelRequest`, `Done`. Planned: `InteractionRequest` (generalizes `PermissionRequested`). Not persisted — `events.jsonl` holds the canonical facts. |
| `RunOutput` | Final result from `.run()` or `UiEvent::Done`: `session_id`, `text`, `usage`, `turn`. |
| `PermissionRequest` | Struct representing a pending permission check: `id`, `tool_call_id`, `tool`, `risk`, `summary`. Emitted via `UiEvent::PermissionRequested`. |
| `PermissionChoice` | Host's response to a permission request: `Once`, `Session`, `Project`, `Deny`. |
| `Error` | Typed error enum (`kuku::error::Error`) covering provider failures, invalid arguments, I/O errors, and prompt rendering failures. |
| `ProviderUsage` | Token usage struct: `input_tokens`, `output_tokens`, `cache_read_input_tokens`, `cache_creation_input_tokens`. Returned in `RunOutput` and `model.response`. |
| `ProviderFailureKind` | Enum classifying provider errors: `Authentication`, `RateLimited`, `ContextTooLarge`, `InvalidRequest`, `ProviderUnavailable`, `Transport`, `Internal`, `Unknown`. |
| `InteractionRequest` | Planned unified host-agent interaction: generalizes `PermissionRequested` to cover ask, confirm, and future interaction types. Host responds via `run.respond()`. |
| `run_start` | Wire event synthesized by the server at run creation (`{"type":"run_start","run_id":"..."}`). Emitted before any `UiEvent`-derived lines. |
| `wire event` | Client-friendly JSON representation of a `UiEvent`, streamed via NDJSON. Produced by SDK's `to_wire()` function. |
| `ExternalToolSource` | (planned) Trait for external tool providers. Implementations include future MCP client. Tools registered through this trait go through the standard permission gate. |
| `progressive disclosure` | Three-stage skill loading: metadata at startup, instructions on trigger, resources on demand. Minimizes context usage. |
| `NDJSON streaming` | Newline-delimited JSON over HTTP. Used by `kuku-server` to stream run events in real time. No SSE, no WebSocket. |
