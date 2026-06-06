# Events

`events.jsonl` is the append-only session fact log. Each line is one persisted fact for a session.

## Naming Rule

```text
<domain>.<action>
```

Lowercase and dot-separated.

## Event Types

| Event | Meaning |
|---|---|
| `session.meta` | Session metadata. First event in a new session. |
| `context.prelude` | Runtime prelude included for context rebuilds. |
| `context.sources` | Context source summary. |
| `turn.start` | Start of one turn. |
| `user.input` | User prompt for the turn. |
| `model.response` | Completed provider response. |
| `model.error` | Provider failure. |
| `tool.call` | One requested tool call. |
| `permission.requested` | Durable pending permission state for one tool call. |
| `permission.allow` | Tool authorization allow decision. |
| `permission.deny` | Tool authorization deny decision. |
| `tool.result` | Result of one tool call. |
| `handoff` | Handoff summary payload. |
| `turn.end` | End of one turn. |
| `turn.rollback` | Rollback marker. |
| `turn.rollback.undo` | Undo of a rollback. |

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

- `conversation_only`
- `files_only`
- `both`

## Permission State

`permission.requested` records that a tool call is waiting for host authorization. It is a durable pending permission state, not an allow or deny decision, and not an observability log record.

When the host resolves the request, kuku appends either `permission.allow` or `permission.deny` before `tool.result`.

## Session Facts vs Runtime Streams

Not every runtime event is a session fact. Streaming deltas such as text chunks, thinking chunks, live tool output, and host-visible log records are runtime stream events, not `events.jsonl` entries.

For HTTP wire events, see [Server API](server-api.md).
