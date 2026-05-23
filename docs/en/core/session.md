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
{"id":1,"type":"session.meta","ts":"...","schema_version":1,"session_id":"20260523-1430-a3f7","created_at":"...","kuku_version":"0.1.0"}
```

Session IDs follow the format `YYYYMMDD-HHmm-xxxx` — local date, 24h time, 4-char hex random suffix.

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

### Status

Every session has one of three statuses (`SessionStatus`):

| Status | Meaning |
|--------|---------|
| `Active` | Lock file exists and holder PID is alive |
| `Done` | No active lock and last event is `turn.end` |
| `Interrupted` | No active lock and last event is not `turn.end` (or no events) |

### Listing sessions

`list_sessions(kuku_home, Option<&Path>)` returns `Vec<SessionSummary>` with session ID, workspace, title (first `user.input`), created_at, turn count, status, mtime, and size. Pass `None` for workspace to list across all workspaces. Results are sorted by mtime descending.

### Deleting a session

`delete_session(kuku_home, Option<&Path>, session_id)` removes the session directory. Returns `Error::SessionLocked` if an active lock is held.

### End

There is no explicit "close". Keep the directory, or `rm -rf` it.
