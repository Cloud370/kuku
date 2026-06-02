# Events

`events.jsonl` is an append-only log. Each line is one persisted event.

## Naming Rule

```text
<domain>.<action>
```

Lowercase and dot-separated.

## Event Types

| Event | Meaning |
|---|---|
| `session.meta` | Session metadata. First event in a new session. |
| `policy.loaded` | Hash of the loaded `policy.md`. Optional. |
| `turn.start` | Start of one turn. |
| `user.input` | User prompt for the turn. |
| `model.request` | Resolved provider request metadata. |
| `model.response` | Completed provider response. |
| `model.error` | Provider failure. |
| `tool.call` | One requested tool call. |
| `tool.result` | Result of one tool call. |
| `permission.request` | Tool awaiting authorization. |
| `permission.decision` | Authorization outcome. |
| `turn.end` | End of one turn. |
| `turn.rollback` | Rollback marker. |
| `turn.rollback.undo` | Undo of a rollback. |
| `handoff.trigger` | Context handoff trigger. |
| `handoff` | Handoff summary payload. |
| `plugin.hook` | Hook execution outcome. |

## Common Fields

Every persisted event line includes at least:

| Field | Meaning |
|---|---|
| `id` | Monotonic integer within the session |
| `type` | Event type |
| `ts` | ISO 8601 timestamp |
| `turn` | Turn number for turn-scoped events |

## Rollback Scope Values

`turn.rollback` records one of these scope values:

- `ConversationOnly`
- `FilesOnly`
- `Both`

## Runtime-Only vs Persisted

Not every runtime event is persisted. Streaming deltas such as text chunks, thinking chunks, and live tool output are host-facing runtime events, not `events.jsonl` entries.

For HTTP wire events, see [Server API](server-api.md).
