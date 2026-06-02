# Sessions

A `Session` is a directory. kuku does not keep a separate session object or database record as the source of truth.

## Layout

Sessions live under the workspace-specific area inside `$KUKU_HOME`:

```text
$KUKU_HOME/p/<workspace-path>/sessions/<id>/
|- lock
|- events.jsonl
|- pre-revert-<id>/
`- subs/
```

`events.jsonl` is the durable history. Child subagent sessions live under `subs/`.

## Event log

Each line in `events.jsonl` is one event. Common event types include:

- `session.meta`
- `turn.start`
- `user.input`
- `model.request`
- `model.response`
- `tool.call`
- `tool.result`
- `turn.end`

Readers trust file order. Trailing partial lines are ignored during recovery.

## Lifecycle

### New session

Starting a run without a session id creates a new session directory and writes `session.meta` before the first turn.

### Continuing a session

Starting a run with an existing session id appends a new turn to that session. kuku rebuilds prior context from the event log.

### Status

Each session is one of:

| Status | Meaning |
|--------|---------|
| `Active` | A live writer lock exists. |
| `Done` | No lock exists and the last event is `turn.end`. |
| `Interrupted` | No lock exists and the last event is not `turn.end`. |

## Writer lock

Only one writer may append to a session at a time. Read operations can happen concurrently.

## Handoff

When context usage exceeds the configured threshold, kuku injects a handoff instruction before the model call. If the model returns a `<kuku_handoff>` document, the runtime stores it in the event log and uses it as the summary boundary for future context rebuilds.

The next request keeps a small number of recent turns and replaces older history with the handoff summary.

## Rollback

Rollback is append-only. kuku records rollback marker events instead of deleting history.

Three scopes exist:

| Scope | Effect |
|-------|--------|
| `ConversationOnly` | Removes prior turns from future context rebuilds. |
| `FilesOnly` | Reverts workspace files to an earlier turn. |
| `Both` | Applies both behaviors. |

File rollback uses snapshots already captured in `tool.result` data and stores pre-revert backups in `pre-revert-<id>/`.

## Session operations

Hosts can list sessions, inspect their events, continue them, or delete them. These are convenience operations around the same on-disk layout.

See [Agent Loop](agent-loop.md) for turn execution and [Host Apps](../architecture/host-apps.md) for how different hosts expose session operations.
