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
{"type":"done","session_id":"...","text":"done","turn":1,"usage":null,"model_request_count":1,"thinking_duration_ms":0,"tool_summary":{"total_calls":0,"names":[],"denied":0,"errors":0,"rounds":0}}
```

The `done` event includes run metrics:

| Field | Type | Description |
|---|---|---|
| `model_request_count` | `u64` | Number of model API calls in this session |
| `thinking_duration_ms` | `u64` | Cumulative time spent in thinking blocks |
| `tool_summary.total_calls` | `u64` | Total tool invocations (including blocked) |
| `tool_summary.names` | `string[]` | Unique tool names in first-appearance order |
| `tool_summary.denied` | `u64` | Permission denials |
| `tool_summary.errors` | `u64` | Tool executions with error status |
| `tool_summary.rounds` | `u64` | Model→tools→result cycles |

On interrupt (`session_interrupted`), `response` is partial text or `null`. `usage` and `tools` use the same structure as above.

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
