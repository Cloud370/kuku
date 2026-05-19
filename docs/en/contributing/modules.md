# Module Contracts

What each module owns, who it depends on, and what it must never do. Architecture overview is in [architecture.md](../core/architecture.md).

## error.rs

Typed error enum covering all SDK failures.

- Provider errors: auth, rate limit, context overflow, network, invalid request.
- Tool errors: invalid arguments, permission blocked, execution failure.
- I/O and config errors.
- `Result<T>` is `Result<T, crate::error::Error>`.

## query/

Public API and agent loop orchestration. The only module that may depend on everything.

- `Query` is a typed builder. `Run` streams `UiEvent` and accepts `decide()` calls.
- `advance_pending` drives the state machine: context → provider → tool dispatch → loop.
- `run()` is a convenience wrapper over `start()` + polling to `Done`.

## event/

`event::store` is the only module that writes to `events.jsonl`.

- Append-only. `EventStore::append` assigns `id` and `ts`, serializes one JSON line, flushes.
- Reader replays in file order. `id` validates monotonicity, not ordering.
- Trailing partial line → ignored with a diagnostic.
- Unknown event types → preserved via two-step deserialization for display, excluded from `messages[]`.

No database abstraction, no event bus, no WAL. This module should be small, boring, hard to misuse.

## session/

Path derivation only. No conversation state, no event interpretation, no message building.

- `$KUKU_HOME` from env or default.
- Project home: `$KUKU_HOME/p/<canonical-workspace-path>/`.
- Session directory: `<project-home>/sessions/<id>/`.
- Writer lock: `<project-home>/runtime/locks/<session-id>.json`.

Full directory layout is in [architecture.md](../core/architecture.md#directory-layout).

## context/

Rebuilds provider-neutral `messages[]` from files and events every turn. Stateless.

Inputs:
- Project instructions (`AGENTS.md` / `CLAUDE.md`)
- Global and project memory
- `events.jsonl` valid chain
- Tool registry snapshot
- Prompt assets

Outputs:
- `ContextAssembly` with system prompt, prelude messages, history, tools, runtime context
- `RequestProvenance` with hashes for instructions, memory, prompts, tool registry, subagent registry

Must not: call a provider, execute a tool, decide permissions, cache anything across turns.

## provider/

Protocol conversion only. Adapters take canonical messages + tool schemas, return normalized model events.

- Input: `CanonicalMessage[]` + `ToolSchema[]` + model config.
- Output: `ModelEvent` stream (TextDelta, ToolUse, Usage, Stop).
- Built-in formats: `anthropic`, `openai-chat`, `openai-responses`.

Must not: own session state, execute tools, decide permissions, write to events.jsonl.

## tool/

Definitions, registry, dispatch, and built-in implementations.

- `ToolDefinition`: name, description, input_schema, risk, read_only, concurrency_safe, max_result_chars.
- `builtin_registry(agent_enabled)` returns 8 or 9 tools in fixed order.
- All results use unified `ToolResultEnvelope`: `{status, summary, model_content, truncated, structured}`.
- Tool schemas are stable and fixed-order. No per-request pruning.

## permission/

Runtime gate decisions.

- Fixed evaluation order: hard guard → policy deny → session grants → policy allow → defaults.
- `hard guard` cannot be overridden by any other source.
- `policy.md` is read on first use; hash recorded in events.
- Three choices exposed to host: `Once`, `Session`, `Project`, `Deny`.

Must not: execute tools, bypass hard guard, treat project instructions as hard permission.

## prompt/

Asset catalog and template rendering.

- `PromptCatalog` owns four assets: system, project_context, tool_guidance, runtime_context.
- Templates use `{{placeholder}}` variables; `system.md` and `tool-guidance.md` are verbatim.
- `load_from_dir()` loads external files; missing files fall back to embedded.
- Prompt text lives in `crates/kuku/prompts/`. Rust code owns catalog, inputs, and rendering.

## config/

Typed config parsing, validation, and writing.

- `Config` is loaded once per session. Changes need a new session.
- `api_key` field: `$VAR` for env reference, otherwise literal.
- Config returned to host is redacted by default.
- Writes are atomic: temp file + rename, with version check for conflicts.

## notice/

Context drift detection and system notice rendering.

- Compares file hashes (memory, instructions) against last acknowledged snapshots from `model.request` provenance.
- Detected drift → `<kuku_system_notice>` injected into `runtime_context`.
- Notice signals change without re-injecting content.

## subagent/

Definitions, registry, catalog rendering, child session spawn.

- `SubagentRegistry` loads from builtins + compatibility imports.
- Catalog is injected into `runtime_context`; full definitions only go to child sessions.
- Child sessions use the same query pipeline with a constrained tool registry.
- V1: shallow only (child sessions do not register the `agent` tool).

## Streaming boundaries

Three distinct event streams:

| Stream | Direction | Persisted |
|--------|-----------|-----------|
| Provider SSE | provider → adapter | No |
| `UiEvent` | SDK → host | No |
| `EventPayload` | SDK → `events.jsonl` | Yes |

Provider stream becomes `UiEvent::TextDelta` in real time. Only after the response completes and is validated does the runtime write `model.response` + `tool.call[]` to `events.jsonl`. A crash mid-stream loses at most the unconfirmed delta.

## Request provenance

Every `model.request` must record:

- Workspace, platform, current date
- Project instruction sources (path + hash)
- Memory sources (path + hash)
- Prompt asset sources (path + hash)
- History event range (first, last, message count)
- Tool registry (hash, names, count)
- Subagent registry (hash, names, if present)
- Resolved provider, model, params, token estimate

No rendered prompt snapshot is stored. Inspection views rebuild from provenance + current files.
