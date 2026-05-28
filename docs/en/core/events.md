# Events

Every event type that can appear in `events.jsonl`. For execution order and cross-event rules, see [agent-loop.md](agent-loop.md).

## Naming

```text
<domain>.<action>
```

Lowercase, dot-separated. Stable domains and a fixed action vocabulary — no synonyms.

| Domain | Actions |
|--------|---------|
| `session` | `meta` |
| `turn` | `start`, `end`, `rollback`, `rollback.undo` |
| `user` | `input` |
| `model` | `request`, `response`, `error` |
| `tool` | `call`, `result` |
| `permission` | `request`, `decision` |
| `policy` | `loaded` |
| `handoff` | `trigger`, (root) |

## Index

| Event | Meaning | Enters `messages[]` | Pairs with |
|-------|---------|---------------------|------------|
| `session.meta` | Session metadata (schema version, session id, created_at, kuku_version). First event. | no | — |
| `policy.loaded` | Hash of the `policy.md` loaded for this session. Optional. | no | — |
| `turn.start` | A turn begins. | no | `turn.end` |
| `user.input` | The human prompt that started this turn. | yes | — |
| `model.request` | Provider request: resolved provider/model, params, provenance. | no | `model.response` or `model.error` |
| `model.response` | Confirmed provider response: text, stop reason, usage. | contributes | `tool.call[]` if `stop_reason: tool_use` |
| `model.error` | Provider failure: auth, rate limit, network, overflow. Diagnostic. | no | `model.request` |
| `tool.call` | A tool use requested by the model. | contributes | `tool.result` |
| `tool.result` | Tool execution outcome: status, summary, model_content, truncated, structured. | contributes | `tool.call` |
| `permission.request` | A tool awaiting authorization. | no | `permission.decision` |
| `permission.decision` | Gate outcome: allow, deny, or deferred to host. | no | `permission.request` |
| `turn.end` | A turn ends. | no | `turn.start` |
| `turn.rollback` | Turn rollback marker: records `target_turn` and `scope` (ConversationOnly, FilesOnly, Both). History is never physically deleted. | no | `turn.rollback.undo` |
| `turn.rollback.undo` | Undoes a rollback. References the `rollback_event_id`. Restores files from backup if safe. | no | `turn.rollback` |
| `handoff.trigger` | Context handoff activated: records trigger reason. | no | `handoff` |
| `handoff` | Handoff summary: text content and number of kept turns. | no (used to build `handoff_summary`) | `handoff.trigger` |

`model.request` provenance records the sources needed to inspect a request: instruction and memory hashes, prompt asset hashes, history range, tool/subagent registry snapshots, resolved provider/model, params, and context budget. It is not a stored provider raw request body.

## UiEvent ↔ persisted event mapping

The runtime emits `UiEvent` to the host and writes `EventPayload` to `events.jsonl`. The two streams are independent — the execution tree cannot be recovered from persisted events alone.

| UiEvent | Persisted event(s) | Notes |
|---------|-------------------|-------|
| `ToolStart` | `tool.call` | Written at slot spawn time |
| `ToolOutput` | — | Runtime-only, not persisted |
| `ToolEnd` | `tool.result` | Written when slot completes; carries `model_content` |
| `ToolOutput` | — | Runtime-only streaming tool output (stdout/stderr/thinking) |
| `PermissionRequested` | `permission.request` | |
| `TextDelta` | — | Runtime-only stream |
| `ThinkingDelta` | — | Runtime-only stream |
| `Done` | `turn.end` | |
| `Error` | `model.error` | When provider fails |
| `TurnStart` | `turn.start` | |
| `ModelRequest` | `model.request` | |
| `Cancelled` | — | Turn cancelled by user; runtime-only |

## Context rebuild

Events marked `contributes` are folded into `messages[]` during context rebuild. Before rebuild, `filter_rolled_back_events()` excludes events from turns that are currently rolled back (respecting scope and undo state). The rebuild then processes remaining events in file order, grouping by `request_id`:

- A `model.response` and its following `tool.call[]` + `tool.result[]` (same `request_id`) form a response group.
- Text and thinking become assistant content blocks. `tool.call` events become `tool_use` blocks in the same assistant message.
- `tool.result` events become user `tool_result` blocks paired to their `tool.call`.
- If a `tool.call` has no matching `tool.result` (crash), a synthetic `status:"cancelled"` result is inserted.
- `turn.end` and `user.input` both flush the current response group before proceeding.
- A `handoff` event truncates the rebuild window: only events after the most recent `handoff` enter `messages[]`. The `handoff.summary` text is returned as `handoff_summary` for the provider to render into context.
