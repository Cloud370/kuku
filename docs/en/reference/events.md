# Events

`events.jsonl` is the append-only ledger for one session. A session is one ledger. A conversation is one chat thread inside that ledger.

## Naming Rule

```text
<domain>.<action>
```

Lowercase and dot-separated.

## Mental Model

- `session`: the durable ledger for one run history
- `conversation`: a chat thread within that ledger
- `agent`: a contact card that can own one or more conversations
- `address`: the conversation identity; reusing it means continuity

## Canonical Event Kinds

| Event | Meaning |
|---|---|
| `session.created` | Ledger metadata. First canonical event in a new session. |
| `conversation.opened` | A conversation address is first opened in the ledger. |
| `conversation.bound` | A conversation is bound to one agent identity snapshot. |
| `prompt.snapshot` | The rebuilt prompt inputs for one conversation turn. |
| `message.user` | A user message inside one conversation. |
| `message.assistant` | An assistant message inside one conversation. |
| `turn.started` | Start of one conversation turn. |
| `turn.completed` | Normal end of one conversation turn. |
| `turn.cancelled` | Cancellation end of one conversation turn. |
| `turn.interrupted` | Interrupted end of one conversation turn. |
| `context.sources` | Instruction and memory source files used for one main-turn rebuild. |
| `context.skills` | Skill registry snapshot and bootstrap-loaded skills for one conversation turn. |
| `tool.call` | One requested tool call. |
| `permission.requested` | Durable pending permission state for one tool call. |
| `permission.allow` | Tool authorization allow decision. |
| `permission.deny` | Tool authorization deny decision. |
| `tool.result` | Result of one tool call. |
| `handoff` | Summary boundary used for future replay. |
| `conversation.rollback` | Conversation-scoped rollback marker. |
| `conversation.rollback.undone` | Undo of a conversation rollback. |

## Common Fields

Every persisted event line includes at least:

| Field | Meaning |
|---|---|
| `id` | Monotonic integer within the session ledger |
| `kind` | Event kind |
| `ts` | ISO 8601 timestamp |

Conversation-scoped events also carry `conversation`. Turn-scoped events also carry `turn`.

## Required Fields By Event

| Event | Required fields |
|---|---|
| `session.created` | `ts`, `schema_version`, `session_id`, `created_at`, `kuku_version` |
| `conversation.opened` | `ts`, `conversation` |
| `conversation.bound` | `ts`, `conversation`, `binding_id` |
| `prompt.snapshot` | `ts`, `conversation`, `binding_id`, `snapshot_id`, `turn`, `messages`, `project_instruction_sources`, `memory_sources`, `prompt_asset_sources`, `skills`, `bootstrap_loaded`, `provider`, `model`, `renderer`, `tool_registry`, `capabilities` |
| `message.user` | `ts`, `conversation`, `turn`, `text` |
| `message.assistant` | `ts`, `conversation`, `turn`, `message_id`, `text` |
| `turn.started` | `ts`, `conversation`, `turn` |
| `turn.completed` | `ts`, `conversation`, `turn` |
| `turn.cancelled` | `ts`, `conversation`, `turn`, `reason` |
| `turn.interrupted` | `ts`, `conversation`, `turn`, `reason` |
| `context.sources` | `turn`, `ts`, `request_id`, `project_instruction_sources`, `memory_sources` |
| `context.skills` | `conversation`, `turn`, `ts`, `registry`, `bootstrap_loaded` |
| `tool.call` | `turn`, `ts`, `tool_call_id`, `request_id`, `index`, `tool`, `args` |
| `permission.requested` | `turn`, `ts`, `tool_call_id`, `tool`, `risk`, `summary`, `candidate`, `source` |
| `permission.allow` | `turn`, `ts`, `tool_call_id`, `tool`, `scope`, `matcher`, `source` |
| `permission.deny` | `turn`, `ts`, `tool_call_id`, `tool`, `reason`, `source` |
| `tool.result` | `turn`, `ts`, `tool_call_id`, `status`, `summary`, `model_content`, `truncated`, `files_read`, `files_changed`, `commands_run` |
| `handoff` | `turn`, `ts`, `request_id`, `summary`, `keep_turns` |
| `conversation.rollback` | `ts`, `conversation`, `to_turn`, `to_event_id`, `scope` |
| `conversation.rollback.undone` | `ts`, `conversation`, `rollback_event_id` |

`tool.call` and `tool.result` optionally include `conversation`. When absent, they belong to the `main` conversation.

## Rollback Scope Values

Rollback events record one of these scope values:

- `messages`
- `file_changes`
- `both`

`messages` removes later events from future replay for that conversation. `file_changes` reverts workspace files without hiding conversation history. `both` does both.

## Permission State

`permission.requested` is a durable pending state, not an observability record. When the host resolves the request, kuku appends either `permission.allow` or `permission.deny` before `tool.result`.

## Session Facts vs Runtime Streams

Not every runtime event is a session fact. Streaming deltas, live command output, tool progress, and host-visible log records are runtime stream events, not `events.jsonl` entries.

For HTTP wire events, see [Server API](server-api.md).
