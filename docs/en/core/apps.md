# Host Apps

<!-- status: partial -->

Host apps present SDK facts to users. Each host depends on `kuku` as a library. No host embeds another host.

## Crate layout

```text
crates/
├── kuku/              # SDK
├── kuku-cli/          # CLI command implementations (lib + standalone bin)
└── kuku-server/       # HTTP server library (lib + standalone bin)

apps/
├── kuku/              # Unified release binary (package: kuku-app, bin: kuku)
├── web/               # Frontend SPA (React 19 + Vite 8, depends on kuku-server)
└── tauri/             # FUTURE — desktop shell (depends on kuku-server)
```

`kuku-cli` and `kuku-server` are independent — neither depends on the other. Both retain their own `[[bin]]` targets for dev-time standalone use. Only `apps/kuku` produces the release artifact.

## Unified CLI

```text
kuku run "check this"                  # agent task
kuku show <session-id>                 # show output
kuku events <session-id>               # show events
kuku list                              # list sessions (current workspace)
kuku list -a                           # list all workspaces
kuku delete <session-id>               # delete a session (with confirmation)
kuku config show                       # show config (redacted)
kuku config validate                   # validate config and report errors
kuku config set model.balanced.think high
kuku init                              # initialize directory structure
kuku prompts show / export <dir>       # prompt assets
kuku agents list / show <name>         # subagent definitions
kuku skills list / show <name>         # skill definitions
kuku server --listen 0.0.0.0:17777     # start HTTP API
kuku server --password <pw>            # with auth
```

No subcommand → interactive mode.

### REPL commands

Inside interactive mode (`kuku run`), slash commands are available:

| Command | Effect |
|---------|--------|
| `/undo` | Roll back to a previous turn (interactive scope picker, file preview, confirmation) |

## Server

`crates/kuku-server` is a long-lived HTTP process. It holds active `Run` instances in memory. No state beyond what the SDK persists to `events.jsonl`.

```text
GET    /health                    health check (returns workspace, version)
POST   /runs                      start a run (NDJSON stream in response body)
DELETE /runs/:id                   cancel a run
POST   /runs/:id/responses        respond to an interaction request
GET    /sessions                  list sessions (?workspace= optional)
DELETE /sessions/:id              delete a session (?workspace= optional)
GET    /sessions/:id/events       historical events (JSON array)
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

### NDJSON streaming

`POST /runs` streams events as newline-delimited JSON in the response body. No SSE, no WebSocket. Standard HTTP with `Transfer-Encoding: chunked`.

```text
← POST /runs { "prompt": "..." }

→ {"type":"run_start","run_id":"..."}
→ {"type":"turn_start","turn":1}
→ {"type":"text","content":"你"}
→ {"type":"text","content":"好"}
→ {"type":"thinking","content":"..."}
→ {"type":"tool_start","id":"...","tool":"find_files","summary":"...","kind":"simple"}
→ {"type":"tool_output","id":"...","event":{"stdout":"..."}}
→ {"type":"tool_end","id":"...","status":"ok","summary":"...","model_content":"..."}
→ {"type":"turn_start","turn":2}
→ {"type":"text","content":"找到了 3 个文件"}
→ {"type":"done","session_id":"...","text":"找到了 3 个文件","turn":2,"usage":{...}}
```

Cancellation produces a `cancelled` event before the stream ends:

```text
→ {"type":"cancelled","turn":1}
```

The SDK provides `to_wire()` to convert `UiEvent` to wire format. The server calls this function; it owns no mapping logic.

### Reconnect

If the connection drops, the client backfills via HTTP then re-subscribes:

```text
1. GET /sessions/:id/events?after=42     ← backfill missed events
2. POST /runs { session_id, prompt }     ← re-subscribe to live stream
```

TextDeltas from the interrupted stream are not replayed. Only structured events (tool calls, permissions) are backfilled.

### Authentication

By default, localhost only (`127.0.0.1`). No authentication.

With `--password`, non-localhost connections require a password:

```text
Client request (no auth)
  → 200 { "ok": false, "code": "auth_required" }
  → WebUI shows password dialog
  → Client retries with Authorization header
```

Localhost connections bypass `--password`.

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

## WebUI

`apps/web` is a frontend SPA. It talks to `kuku-server` via `fetch` (POST for runs, GET for history). The same SPA works behind `apps/tauri` via localhost HTTP.

Key interactions:

- **Run**: `POST /runs`, read NDJSON stream via `fetch` + `ReadableStream`
- **Permission**: interaction event in stream → modal dialog → `POST /runs/:id/responses`
- **Cancel**: `DELETE /runs/:id`
- **History**: `GET /sessions/:id/events`

## Tauri (planned)

`apps/tauri` is a desktop shell. The Tauri backend depends on `kuku-server` as a library (embedded HTTP), not on `kuku` SDK directly. The webview loads the same SPA as `apps/web`, talking to localhost HTTP.

```text
Tauri app startup:
  1. kuku_server::start_server(config, password, max_concurrent_runs) → localhost:{port}
  2. webview.load_url("http://localhost:{port}/")
```

No IPC, no process management — just embedded HTTP server + webview. Distributed independently (.dmg / .msi / .AppImage), not bundled with CLI.

## Wire events

All SDK events are converted to wire format and streamed via NDJSON. The client ignores events it does not need. Wire format is defined in [evolution.md](evolution.md#wire-format). Event types use underscore notation to distinguish from persisted events (dot notation) and Rust variants (PascalCase).
