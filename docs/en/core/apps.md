# Host Apps

<!-- status: design -->

Host apps present SDK facts to users. Each host is an independent binary that depends on `kuku` as a library. No host embeds another host.

## Host structure (planned)

```text
apps/
├── terminal/     bin crate — interactive CLI, stdin/stdout
├── server/       bin crate — HTTP API, NDJSON streaming
└── web/          frontend SPA — consumes server API
```

Future hosts: `apps/tauri/` (desktop app, direct SDK dependency, localhost HTTP).

Each host calls the SDK directly. No shared host-layer crate.

## Server (planned)

`apps/server` is a long-lived HTTP process. It holds active `Run` instances in memory. No state beyond what the SDK persists to `events.jsonl`.

```text
POST   /runs                       start a run (NDJSON stream in response body)
POST   /runs/:id/responses         respond to an interaction request
DELETE /runs/:id                   cancel a run
GET    /sessions                   list sessions
GET    /sessions/:id               session metadata
GET    /sessions/:id/events        historical events (JSON array)
GET    /sessions/:id/events?after=N  events after id N (reconnect backfill)
GET    /sessions/:id/diff          session file changes (git diff)
GET    /workspace/files            file tree and content (read-only)
GET    /agents                     available agent list
GET    /config                     resolved config (redacted)
```

### Workspace

The client specifies `workspace` per request. If omitted, the server defaults to its own working directory.

```json
POST /runs
{
  "prompt": "check this project",
  "workspace": "/code/my-project"
}
```

Multiple workspaces are supported from day one. No restart required to switch.

### NDJSON streaming (planned)

`POST /runs` streams events as newline-delimited JSON in the response body. No SSE, no WebSocket. Standard HTTP with `Transfer-Encoding: chunked`.

```text
← POST /runs { "prompt": "..." }

→ {"type":"turn_start"}
→ {"type":"text","content":"你"}
→ {"type":"text","content":"好"}
→ {"type":"tool_start","id":"...","name":"find_files","summary":"..."}
→ {"type":"tool_end","id":"...","name":"find_files","status":"ok","summary":"..."}
→ {"type":"turn_start"}
→ {"type":"text","content":"找到了 3 个文件"}
→ {"type":"done","usage":{...}}
```

The SDK provides `to_wire()` to convert `UiEvent` to wire format. The server calls this function; it owns no mapping logic.

### Reconnect

If the connection drops, the client backfills via HTTP then re-subscribes:

```text
1. GET /sessions/:id/events?after=42     ← backfill missed events
2. POST /runs { session_id, prompt }     ← re-subscribe to live stream
```

TextDeltas from the interrupted stream are not replayed. Only structured events (tool calls, permissions) are backfilled.

### Authentication (planned)

By default, localhost only (`127.0.0.1`). No authentication.

With `--passwd`, non-localhost connections require a password:

```text
Client request (no auth)
  → 200 { "ok": false, "code": "auth_required" }
  → WebUI shows password dialog
  → Client retries with Authorization header
```

Localhost connections bypass `--passwd`.

### Error model

HTTP status is always 200. Business errors use the response envelope:

```json
{ "ok": false, "code": "session_locked", "message": "...", "details": { "pid": 1234 } }
```

Stream errors are NDJSON events:

```json
{"ok":false,"type":"error","code":"provider_rate_limit","message":"..."}
```

Only infrastructure failures (server down, route not found) use HTTP status codes (503, 404).

### Error codes

| Code | Meaning |
|------|---------|
| `invalid_request` | Bad parameters |
| `session_locked` | Concurrent write conflict |
| `session_not_found` | Session does not exist |
| `agent_not_found` | Agent does not exist |
| `auth_required` | Password needed (remote access) |
| `provider_auth` | Upstream provider auth failure |
| `provider_rate_limit` | Upstream rate limited |
| `provider_network` | Upstream connection failure |
| `provider_overflow` | Context window exceeded |
| `internal` | Server internal error |

## WebUI (planned)

`apps/web` is a frontend SPA. It talks to `apps/server` via `fetch` (POST for runs, GET for history). The same SPA works behind `apps/tauri` via localhost HTTP.

Key interactions:

- **Run**: `POST /runs`, read NDJSON stream via `fetch` + `ReadableStream`
- **Permission**: interaction event in stream → modal dialog → `POST /runs/:id/responses`
- **Cancel**: `DELETE /runs/:id`
- **History**: `GET /sessions/:id/events`
- **File viewer**: `GET /workspace/files` (read-only)
- **Diff viewer**: `GET /sessions/:id/diff`

## Tauri (planned)

`apps/tauri` is a desktop app. The Tauri backend depends on `kuku` directly (no sidecar). The webview loads the same SPA as `apps/web`, talking to localhost HTTP.

## Wire events (planned)

All SDK events are converted to wire format and streamed via NDJSON. The client ignores events it does not need. Wire format is defined in [evolution.md](evolution.md#wire-format-planned). Event types use underscore notation to distinguish from persisted events (dot notation) and Rust variants (PascalCase).
