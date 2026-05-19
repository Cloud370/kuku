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
| `turn` | `start`, `end` |
| `user` | `input` |
| `model` | `request`, `response`, `error` |
| `tool` | `call`, `result` |
| `permission` | `request`, `decision` |
| `policy` | `loaded` |

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

## Context rebuild

Events marked `contributes` are folded into `messages[]` during context rebuild. The rebuild processes events in file order, grouping by `request_id`:

- A `model.response` and its following `tool.call[]` + `tool.result[]` (same `request_id`) form a response group.
- Text and thinking become assistant content blocks. `tool.call` events become `tool_use` blocks in the same assistant message.
- `tool.result` events become user `tool_result` blocks paired to their `tool.call`.
- If a `tool.call` has no matching `tool.result` (crash), a synthetic `status:"cancelled"` result is inserted.
- `turn.end` and `user.input` both flush the current response group before proceeding.
