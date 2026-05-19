# Session

A `session` is a directory. There is no session object, state machine, or database — the directory is the session.

Sessions are scoped to a `workspace`. A session under `/home/user/projects/my-app` lives at:

```text
$KUKU_HOME/p/<workspace-path>/sessions/<id>/
├── events.jsonl
└── subs/                  (child sessions, if subagents ran)
```

Full `$KUKU_HOME` layout is in [architecture.md](architecture.md).

Inspection, transcript, and display views are derived from `events.jsonl`; they are not separate session state.

## events.jsonl

A single append-only file. One JSON line per event. Every fact the runtime observes — model requests, tool calls, permission decisions — is a line in this file.

- `grep` it directly. No need to `ls` then `cat`.
- The last complete line is the current position.
- Reader ignores trailing partial lines (crash recovery).

### Common fields

Every line has at minimum:

| Field | Meaning |
|-------|---------|
| `id` | Monotonic integer within the session |
| `type` | Event type: `session.meta`, `user.input`, `model.request`, etc. |
| `turn` | Which turn this event belongs to (omitted for session-level events) |
| `ts` | ISO 8601 timestamp |

Events are replayed in file order. `id` validates monotonicity; `ts` is display-only.

### session.meta

The first event in every new session:

```jsonl
{"id":1,"type":"session.meta","ts":"...","schema_version":1,"session_id":"s_001","created_at":"...","kuku_version":"0.1.0"}
```

## Writer lock

Only one writer per session at a time. A `lock` file (containing `pid` and
`timestamp`) lives inside the session directory. Read operations (`show`,
`inspect`, `list`) can run concurrently.

If a stale lock is taken over, a diagnostic event is appended to `events.jsonl`.

## Lifecycle

### New session

`kuku::query(prompt).run()` with no session id → new directory under `<project-home>/sessions/<id>/`, `session.meta` appended, then the first turn begins.

### Continuing a session

`kuku::query(prompt).session(id).run()` — appends a new turn to the existing session. Context rebuild picks up prior history automatically.

### End

There is no explicit "close". Keep the directory, or `rm -rf` it.
