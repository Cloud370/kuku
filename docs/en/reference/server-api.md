# Server API

The terminal, script and browser clients use the same HTTP Session contract. The server owns execution and resolves workspaces, model selection and provider credentials on its machine. Observers can disconnect and return while execution continues. For local service discovery and explicit server setup, see [Run the server](../guides/run-the-server.md).

## Authentication and versions

Authenticated clients send `Authorization: Bearer TOKEN`, including on loopback. For managed local launches, `kuku web` opens the embedded browser with a short-lived, single-use ticket in the URL fragment. The page removes the fragment and exchanges the ticket for a separate browser bearer token, stored in that tab's `sessionStorage`. Reloading the same tab keeps its connection credentials for that service instance. An explicitly started service uses its configured access token. The default server tool policy trusts the authenticated user and permits file and command tools.

`GET /health` is public and reports `version`, `protocol_version` and `management_version`. The protocol version describes the Session API. When upgrading a managed server, you can still check service status and request an orderly restart if the Session API versions differ. Missing or rejected credentials return `401` with `auth_required`. A managed server also checks its loopback host and the supplied browser origin.

## Endpoints

| Method | Path | Purpose |
|---|---|---|
| `GET` | `/health` | Build and protocol versions |
| `GET` | `/service/status` | Effective service identity and activity |
| `POST` | `/service/restart` | Ask a managed service to release its Sessions and stop for orderly replacement |
| `POST` | `/service/browser-ticket` | Issue an authenticated one-time browser handoff ticket |
| `POST` | `/service/browser-session` | Exchange a ticket on the managed browser origin |
| `GET` | `/sessions` | List retained Sessions, optionally filtered by `workspace` or `search` |
| `DELETE` | `/sessions/{id}` | Delete a closed Session |
| `POST` | `/sessions/{id}/commands` | Submit a durable command |
| `GET` | `/sessions/{id}/receipts/{command_id}` | Look up a committed command receipt |
| `GET` | `/sessions/{id}/agents` | Summarize Agents in the workspace's Session |
| `GET` | `/sessions/{id}/snapshot` | Current Agent and queue state at a committed cursor |
| `GET` | `/sessions/{id}/history` | Read conversation entries before a cursor |
| `GET` | `/sessions/{id}/observe` | Follow committed entries and transient progress after a cursor |
| `GET` | `/sessions/{id}/events` | Page through raw committed facts |
| `GET` | `/sessions/{id}/outputs/{output_id}` | Read bounded retained tool output |
| `GET` | `/sessions/{id}/jobs` | List the selected Agent's background Jobs |
| `GET` | `/sessions/{id}/jobs/{job_id}` | Inspect a Job and its available result |
| `DELETE` | `/sessions/{id}/jobs/{job_id}` | Cancel a Job and return its terminal status |
| `GET` | `/sessions/{id}/values` | List the selected Agent's named values |
| `PUT` | `/sessions/{id}/values/{name}` | Bind a named value or explicitly replace its source |
| `GET` | `/models` | Configured model catalog |
| `GET` | `/models/resolve` | Resolve effective settings for a proposed Agent configuration |
| `GET` | `/workspaces` | List workspaces |
| `GET` | `/workspaces/resolve`, `/workspaces/files`, `/workspaces/skills` | Resolve a server path or discover workspace files and Skills |

The status response includes the build and protocol versions, `instance_id`, `pid`, effective `data_dir` and `config_path`, `draining`, and `busy`. For a managed restart, `202` acknowledges that writers have been released and the process is stopping. `409` with `service_busy` includes active work such as running Jobs in `busy`. The managed client starts the replacement after the previous process releases its service lease.

## Commands and receipts

Choose a unique Session ID and command ID before the initial send. The body includes the absolute path to the workspace on the server, the target Agent address and the command identity. Include `create` with initial Agent configuration when sending to a new Session.

```json
{
  "workspace": "/code/my-project",
  "agent": "main",
  "command_id": "input-389cd599",
  "type": "send",
  "text": "Summarize this directory",
  "timing": "steer",
  "create": {}
}
```

Use `create: {}` for the configured default settings. Supply initial model settings inside `create` or send an explicit `configure` command for an existing Agent. An Agent keeps its own retained context and settings across clients. Model selection uses the [SDK resolver](sdk.md#operations).

| Command `type` | Additional fields | Effect |
|---|---|---|
| `send` | `text`, optional `timing`, `create`, and `values` | Durable delivery. The default `steer` allows the next safe model-request boundary. `next_turn` waits for the next Turn. |
| `create_agent` | `config` | Create the addressed Agent with independent context. |
| `configure` | `config` | Change model or defaults at a safe boundary. |
| `reply_question` | `request_id`, `text` | Answer the exact pending question. |
| `reply_permission` | `request_id`, `allow` | Decide the exact pending authorization. |
| `withdraw` | `input_command_id` | Withdraw a queued input or return its current status. |
| `replace_queued` | `input_command_id`, `text` | Replace text before admission while preserving the queue position and timing. |
| `advance` | | Explicitly resume an Agent. |
| `cancel` | `execution_id` | Cancel the observed execution and await cleanup. |

A successful command returns JSON with `ok`, `receipt` and `replayed`. `receipt` includes `command_id`, `event_id` and `agent`. Input commands can report `status` as `queued`, `accepted` or `withdrawn`. A durable receipt acknowledges delivery. Model acceptance and execution settlement are separate facts.

To pair an existing Agent's new input with selection changes, send `config_patch`. To let a script create an absent Agent in an existing Session, also send `create_agent_if_missing: true`. For a new Session, use `create` for its initial Agent and optional `prompts_dir` for an absolute path to custom Prompt assets on the server.

Use `bootstrap_skill` on `send` to attach a Skill to that input at its admission boundary.

`replace_queued` records a new receipt and marks the original input as withdrawn in the same commit. If the original input is already admitted or otherwise absent from the queue, the command returns `409` with `input_not_queued`.

If a response is lost, query the receipt endpoint with `workspace`. A missing receipt returns `404` and `receipt_not_found`. If you resend, keep the original Session, Agent, command ID and payload. Replaying a committed command returns the original receipt. Reusing a command ID for a conflicting intent returns HTTP `409`. `?replay_only=true` validates a candidate replay and returns `receipt_not_found` if the command has yet to commit.

Client errors use `{"ok":false,"code":"...","message":"..."}`. Invalid requests return `400`, absent resources `404`, conflicting operations `409`, and an exhausted execution limit `429`. A stale execution identity or mismatched reply is a conflict. Retain the original command identity when retrying an uncertain delivery.

## Snapshot, history and observation

1. Request `/snapshot?workspace=...&agent=...`. The JSON response includes `session_id`, `workspace`, the committed `cursor`, a selected `agent` snapshot, an `agents` summary, `pending` inputs and the model `selection`. A selection failure appears as `selection_error`. Agent state carries the execution identity and pending question or permission where applicable. Each pending item includes its `receipt` and `input`. A missing Agent returns `404` with `agent_not_found`. `/agents?workspace=...` provides all Agent summaries and a cursor without selecting a recipient. Add `progress=true` to request the observed phase at the snapshot cursor. `progress.phase` can be `waiting_for_model`, `thinking`, `streaming`, `tool_arguments` or `running_tools`. `running_tools` includes `tool_call_ids`.
2. Request `/history?workspace=...&agent=...&before=CURSOR_PLUS_ONE&limit=...` for the most recent page through that snapshot, in ascending order. Use the returned `next_before` for an earlier page. `before` is exclusive.
3. Connect to `/observe?workspace=...&agent=...&after=CURSOR` for NDJSON with commits after the snapshot. Add `include_agent_activity=true` with a selected Agent to receive sibling activity in the same subscription. Sibling text deltas and settlement previews are bounded, and sibling tool output is omitted. Agent summaries provide the last output or error preview when reconnecting. Omit `agent` to observe Session-wide. Update your resume cursor from committed records. Reconnect after the last committed cursor on disconnect or lag.

The selected Agent’s `agent.usage` contains cumulative token counts and counts of requests that reported input or output. `cache_measured_input_tokens` and `cache_measured_read_tokens` sum input and cache reads over the same requests with valid paired counts. When available, `latest_request` carries `input_tokens`, `output_tokens`, `context_window_tokens` and `elapsed_us` for one request. Optional measurements may be absent on older records. [Terminal usage fields](cli.md#terminal-interface)

When available, `agent.turn_started_at_ms` is the latest Turn's first admission time in milliseconds since the Unix epoch. After settlement, `agent.turn_elapsed_ms` gives its final elapsed time in milliseconds. The committed `inputs_accepted` frame for a new Turn can also carry `turn_started_at_ms`.

`committed` lines contain `id` and `kind`, plus an Agent address, conversation entry or state-change indicator when applicable. `live` lines carry `agent`, `execution_id`, `turn`, `cursor` and a transient `event`. Match those identities to the current view and replace previews when committed results arrive. A `lagged` line carries `skipped` transient observations. The stream can also emit `heartbeat` or `error` lines. Durable state remains readable after lag.

When present, a committed frame's `delivery` associates queue changes with command IDs. `accepted` carries `input_ids`, `withdrawn` carries `input_id`, and `replaced` carries `input_id` and `replacement_id`.

A snapshot requested with `progress=true` may include `reply_preview` for the selected Agent. It carries `execution_id`, `turn`, optional `turn_number`, `phase`, `text` and `truncated`. The phase is `thinking`, `streaming` or `tool_arguments`. The text is a UTF-8 prefix of the active request's visible reply capped at 64 KiB. `truncated` means the reply has already exceeded the retained prefix. An optional `reasoning_preview` carries a bounded prefix of provider-visible reasoning text or a published summary for the selected Agent. Live `reasoning_preview` events append to it. Committed assistant entries can carry the same field.

At the same snapshot cursor, other Agents can also have `progress.phase` in their entries in `agents`. A `streaming` entry includes `progress.preview`, up to 512 UTF-8 bytes from the start of that Agent's active reply. Phases `thinking` and `tool_arguments` carry activity only.

On an Agent-scoped attachment with `include_agent_activity=true`, the stream can also send a `live` line with `event.type` set to `reply_preview`, the phase in `event.phase`, the prefix in `event.content` and the limit flag in `event.truncated`. Replace the matching in-memory preview with this prefix, then apply later deltas. The stream captures the prefix and begins following new deltas at the same attachment boundary. In this scope, live `thinking` and `tool_arguments` events indicate activity only. Tool execution begins at committed dispatch. Only committed records advance the resume cursor.

History entries carry a durable `id`, Agent address, optional Turn identity, its per-Agent display ordinal (`turn_number`) when known, and a `type` such as `user`, `assistant`, `tool_call`, `tool_result`, `question`, `permission`, `question_reply`, `permission_reply`, `status` or `error`. A tool result can include a summary, structured details and a retained `output_id`. A committed frame can also carry the runtime `operation` or a `settlement` with the Turn result.

For a `user` entry, `agent` identifies the recipient. When trusted runtime input records an Agent sender, `from` contains that sender's Session-local address, including on the input that opens a Turn.

An explicit interruption can produce a `status` entry with `status: "interrupted"`, the `execution_id`, and the Turn identity and number when available. Its `text` holds up to 64 KiB of the interrupted reply, or is `null` if no visible text was retained. `truncated` indicates a longer partial reply. The raw journal can link its `turn.cancelled` event to the stop record with `stop_event_id`.

A settled Turn can have a `turn_summary` entry. When a Turn is explicitly interrupted, its interrupted `status` entry can carry the same summary. The `turn_summary` object has `tool_calls`, the count of committed tool calls including calls awaiting authorization. Available measurements include `elapsed_ms` from admission to settlement, `input_tokens` and `output_tokens` when all model requests reported the corresponding count, and `cache_hit_percent` over requests with paired cache-read and input counts.

Agent snapshots and summaries can include `admitted_task` and `queued_task`. These nullable strings are bounded excerpts of the latest admitted task input and the latest task input still queued, respectively. Replies to questions are excluded from task excerpts. Use `pending_inputs`, `question` and `permission` for queue and decision state.

To answer `ask_user` or a deferred tool authorization, use the request ID from the snapshot. Competing replies are checked against the exact pending request. Reopening a Session preserves pending requests and queued inputs. A dispatch without a confirmed result may have changed files or triggered a side effect. Inspect the workspace before continuing interrupted work. [Sessions](../how-it-works/sessions.md)

## Raw events and retained tool output

`/events?workspace=...&after=0&limit=100` returns `events` and `next_after`. The raw event page is for auditing. Each record has an `id` and `payload`. An optional `agent` filter applies within the scanned page, and `next_after` advances even if that page has no matches. See [Session events](events.md) for the journal schema.

Read a known tool output by its ID in `/outputs/{output_id}`, passing `workspace` and optionally `agent` (default `main`), `offset` and `max_bytes`. The Agent must own or have received the output. Use `search` for a literal match within the range. The response wraps the chunk in `output`. Pass `output.next_offset` to continue when `output.has_more` is true. [Retained output](tools.md)

## Background Jobs

All Job routes require `workspace` and `agent` query parameters. `GET /sessions/{id}/jobs` returns `{ "jobs": [...] }` with metadata for the Agent's recent Jobs and all running Jobs. A Job lookup returns the snapshot directly, including the terminal `result` when available. Use `output_id`, when present, to read the retained command output through the output endpoint. An unknown Job ID or a Job belonging to another Agent returns `404` with `job_not_found`.

`DELETE /sessions/{id}/jobs/{job_id}` waits for the targeted execution to settle and returns the resulting snapshot. Repeating the request for a terminal Job returns its existing result. Both GET routes can read a closed Session without opening it. A DELETE for an unsettled Job in a Session that this server has not opened returns `409` with `job_not_active`.

The owning Agent's history and observation stream include a `job_finished` entry with `job_id`, `status`, `summary`, the original `turn` and any `output_id`. This entry can arrive after its Turn settles.

## Named values

`GET /sessions/{id}/values?workspace=...&agent=...` returns `{ "values": [...] }`. Each item contains `name`, `kind`, and `reference` (`id`, `offset`, `bytes`). Read its payload through the output endpoint with the same Agent.

`PUT /sessions/{id}/values/{name}` takes a JSON body with `workspace`, `agent`, `source`, and optional `replace` (default `false`). The source object carries `kind` and `value`. Supported kinds are `text` for a UTF-8 string, `json` for a JSON value, `bytes` for an array of byte values, and `output` for an existing output ID. For a new Session, pass `create` with the initial `main` Agent configuration to save before the first input. The response contains the saved value metadata.

On `send` to an existing Agent, `values` can carry the selected `reference` objects. The command binds those exact references to the submitted input. [Named Session values](tools.md#named-session-values)

## Discover models and workspaces

`/models` returns `default_tier`, `connections` and `tiers`. A connection identifies its `name` and wire `protocol`. A tier identifies its `name`, `connection`, `model` and `purpose`, together with settings and capabilities or a resolution error. Pass model selection fields such as `tier`, `connection`, `model`, `reasoning_effort` or `service_tier` to `/models/resolve` to get a resolved `selection`.

`/workspaces` returns `workspaces`. Pass an existing absolute directory as `workspace` to `/workspaces/resolve` to obtain the canonical path. `/workspaces/files` and `/workspaces/skills` also accept a `query` filter and return `paths` and `skills`, respectively. File matches are workspace-relative paths and Skill matches include names and descriptions.
