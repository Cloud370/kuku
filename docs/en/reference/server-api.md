# Server API

## Start the Server

The default address is `127.0.0.1:17777`. Non-loopback listeners require `--password`.

## Authentication

Loopback clients bypass password checks. Remote clients send `Authorization: Bearer <token>`. Failed authentication returns HTTP 200 with `{"ok":false,"code":"auth_required","message":"password required"}`.

The default single-user server uses trusted-local tool authorization. Embedded hosts can provide `AppState.authorizer` to allow, deny, or defer exact tool actions.

## Endpoints

| Method | Path | Purpose |
|---|---|---|
| `GET` | `/health` | Health check |
| `POST` | `/runs` | Submit work and observe NDJSON |
| `DELETE` | `/runs/{id}` | Cancel and await owned cleanup |
| `POST` | `/runs/{id}/responses` | Answer a pending question or authorization |
| `POST` | `/sessions/{id}/responses` | Reopen a persistent Session and answer its exact pending request |
| `GET` | `/sessions` | List retained Sessions |
| `DELETE` | `/sessions/{id}` | Delete a closed Session |
| `GET` | `/sessions/{id}/events` | Read a bounded durable page and optional recent progress |
| `GET` | `/sessions/{id}/conversations` | Inspect stable Agents inside one Session |

Request bodies are limited to 10 MB. SDK input and execution limits still apply.

## `GET /health`

```json
{"ok":true,"version":"0.1.0","workspace":"/server/working/directory"}
```

## `POST /runs`

```json
{
  "prompt": "check this project",
  "workspace": "/code/my-project",
  "session_id": "optional-session",
  "agent": "reviewer",
  "connection": "named-connection",
  "model": "exact-model-id",
  "command_id": "optional-caller-command"
}
```

`workspace` must be a directory. `session_id` is optional; `agent` defaults to `main`. Addresses are flat, stable lowercase ASCII letters, digits, `-`, or `_`, up to 64 bytes. `conversation` remains an input alias for `agent`.

Model selection uses the SDK resolver. Optional fields are `tier`, `connection`, `model`, `reasoning_effort`, `service_tier`, and `max_output_tokens`. `no_agents` and `no_skills` disable their tool surfaces. An exact connection selects its configured endpoint regardless of protocol overlap. Reopening without model options preserves that Agent's configuration; explicit changes affect only that Agent. A changed route clears previous Agent-local reasoning/service overrides before applying new explicit values.

`command_id` accepts 1–100 bytes and is the SDK input's idempotency identity. The returned `run_id` identifies this HTTP execution handle; it is separate from the Session-local command identity. Receipt acknowledges durable delivery, while terminal execution facts arrive later. Reuse the same Session and Agent to continue its context. Other addresses in the Session have independent contexts and may execute concurrently. New commands for an already-running or waiting address fail visibly. Retrying the same input command returns HTTP JSON with `ok`, `replayed: true`, its original durable `receipt`, and the retained `run_id` when still available. This retry does not allocate another execution slot, alter configuration, or dispatch work again. Conflicting input or explicit configuration reuse fails. SDK callers can still enqueue fresh inputs at safe runtime boundaries; the HTTP host keeps one active control handle per Agent.

The response uses `application/x-ndjson`. When the last HTTP run finishes, the server waits for Agents already running in that Session before closing it. Waiting and interrupted Agents stay dormant. Session execution continues when the HTTP observer disconnects. Closing or slowing a client does not control the execution chain.

## NDJSON Wire Events

`run_start` contains `run_id`, `session_id`, `agent`, and the durable input `receipt`. Live updates include `turn_start`, `model_request`, `text`, `thinking`, `tool_start`, `tool_output`, `tool_end`, and `log`.

`committed` identifies a durable event by `id` and `kind`; read its payload through the event endpoint. `lagged` reports dropped transient updates; reconnect using the durable event cursor. Queues are bounded: 256 broadcast messages and 200 recent messages, capped at 1 MiB per run. Live progress may be lost; committed facts and the terminal response remain inspectable through the Session log.

A question emits `question` with its exact `id`, `agent`, and `text`. A deferred authorization emits `permission` with its matching request identity. Each pending request identity is emitted once per retained HTTP control, including when a question and authorization coexist. Answering the question does not decide the authorization. Waiting releases the server execution slot; it does not time out into implicit approval or denial.

Terminal events are `done`, `cancelled`, or `error`. `done` follows durable settlement and includes `session_id`, `agent`, `text`, `turn`, cumulative provider `usage`, and the Agent's `config`. `conversation` mirrors the Agent address for existing consumers. A completed Turn does not establish task success; inspect actual files and verifier commands.

```json
{"type":"run_start","run_id":"http-run-1","session_id":"...","agent":"main","receipt":{"command_id":"command-1","event_id":3,"agent":"main"}}
{"type":"text","content":"hello","agent":"main"}
{"type":"done","session_id":"...","agent":"main","conversation":"main","text":"done","turn":1,"usage":{"model_requests":1},"config":{}}
```

Provider usage and unknown reporting follow the [Session SDK](../how-it-works/sessions.md). Active `log` updates are observability records, separate from persisted execution facts.

## `DELETE /runs/{id}`

Success returns `{"ok":true}` after cancellation and cleanup have completed. A missing run returns `session_not_found`; cleanup errors return `cleanup_failed` and their message. Server shutdown closes Sessions, preserves pending waits, awaits owned cleanup, and reports failures.

## `POST /runs/{id}/responses`

```json
{"interaction_id":"exact-pending-request","agent":"reviewer","choice":"once","command_id":"reply-1"}
```

`choice` is `once` or `deny` for authorization, or `answer` with a `text` field for a question. `agent` is optional but must match the original run when supplied. `conversation` remains its alias. Session/project-wide grants are not inferred from a reply. A caller-selected `command_id` gives idempotent reply retries. Mismatched or stale request identities fail without advancing the Agent.

Replies to a currently open server run continue through its original stream.

## `POST /sessions/{id}/responses`

Use the persistent Session identity after a server restart; no old HTTP `run_id` is needed. The body uses the same reply fields and requires `workspace`:

```json
{"workspace":"/code/my-project","agent":"reviewer","interaction_id":"exact-pending-request","choice":"answer","text":"the requested information","command_id":"reply-1"}
```

The Session must already exist. `agent` defaults to `main`. The host opens that Session, calls its matching SDK reply operation, and retains the same runtime until its continuation settles or waits again. A new HTTP execution returns the normal NDJSON stream and a new `run_id`; `run_start.receipt` identifies the durable reply. If this Agent already has a retained HTTP control, the reply returns JSON acknowledgement with that `run_id` and continues through its existing stream. Duplicate reply commands return their original receipt without creating another continuation. Mismatched/stale requests and conflicting retries fail without model or tool dispatch.

## `GET /sessions`

An optional `workspace` query filters the list. Response: `{"ok":true,"sessions":[]}`. Each Session includes `session_id`, `workspace`, `title`, `created_at`, `turn_count`, `status`, `mtime`, and `size`.

## `GET /sessions/{id}/conversations`

An optional `workspace` selects the Session location. This retained endpoint returns `{"ok":true,"session_id":"...","conversations":[]}`. Each item reports `agent`/`conversation`, `status`, `pending_inputs`, pending `permission`/`question`, `config`, and `usage`. Reading does not advance execution or acquire another Session runtime.

## `DELETE /sessions/{id}`

An optional `workspace` selects the location. Common errors are `session_locked`, `session_not_found`, and `internal`.

## `GET /sessions/{id}/events`

Query fields:

- `after`: exclusive durable event cursor, default 0.
- `limit`: maximum scanned records, default 100; accepts 1–256.
- `agent` or `conversation`: optional address filter within the bounded ledger page.
- `workspace`: Session location.

The response always contains `events`, `next_after`, and `active_stream`. `next_after` is the last scanned ledger cursor, even when filtering excludes every event. Use it for the next page. Stored events retain their `id` and `payload`; active recent progress is optional and never replaces durable records.

A separate process holding the Session writer may prevent a read; that resource conflict is returned visibly rather than starting another writer.
