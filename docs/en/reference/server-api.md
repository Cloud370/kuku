# Server API

## Start the Server

Default listen address:

```text
127.0.0.1:17777
```

Non-loopback listeners require `--password`.

## Authentication

- Loopback clients bypass password checks.
- Remote clients must send `Authorization: Bearer <token>`.
- Failed auth returns HTTP 200 with:

```json
{"ok": false, "code": "auth_required", "message": "password required"}
```

## Endpoints

| Method | Path | Purpose |
|---|---|---|
| `GET` | `/health` | Health check |
| `POST` | `/runs` | Start a run and stream NDJSON |
| `DELETE` | `/runs/{id}` | Cancel a run |
| `POST` | `/runs/{id}/responses` | Reply to an interaction request |
| `GET` | `/sessions` | List sessions |
| `DELETE` | `/sessions/{id}` | Delete one session |
| `GET` | `/sessions/{id}/events` | Read persisted events and optional active stream |

Request bodies are limited to 10 MB.

## `GET /health`

Response:

```json
{
  "ok": true,
  "version": "0.1.0",
  "workspace": "/current/server/working/dir"
}
```

## `POST /runs`

Request body:

```json
{
  "prompt": "check this project",
  "workspace": "/code/my-project",
  "session_id": "optional-existing-session",
  "tier": "optional-tier-name"
}
```

Rules:

- `workspace` is required and must exist
- `session_id` is optional
- `tier` is optional

Success response: NDJSON stream with content type `application/x-ndjson`.

## NDJSON Wire Events

Top-level event types currently emitted by the server:

- `run_start`
- `turn_start`
- `model_request`
- `text`
- `thinking`
- `tool_start`
- `tool_output`
- `tool_end`
- `permission`
- `done`
- `cancelled`
- `error`

Examples:

```json
{"type":"run_start","run_id":"..."}
{"type":"text","content":"hello"}
{"type":"done","session_id":"...","text":"done","turn":1,"usage":null}
```

## `DELETE /runs/{id}`

Responses:

```json
{"ok": true}
```

or:

```json
{"ok": false, "code": "session_not_found", "message": "run not found"}
```

## `POST /runs/{id}/responses`

Request body:

```json
{
  "interaction_id": "req_1",
  "choice": "once"
}
```

Valid `choice` values:

- `once`
- `session`
- `project`
- `deny`

## `GET /sessions`

Query:

- optional `workspace`

Success response:

```json
{
  "ok": true,
  "sessions": []
}
```

Each session item includes `session_id`, `workspace`, `title`, `created_at`, `turn_count`, `status`, `mtime`, and `size`.

## `DELETE /sessions/{id}`

Query:

- optional `workspace`

Common error codes:

- `session_locked`
- `session_not_found`
- `internal`

## `GET /sessions/{id}/events`

Query:

- optional `after` integer event id
- optional `workspace`

Response shapes:

- historical events only: JSON array
- historical events plus live buffered lines: object with `events` and `active_stream`
