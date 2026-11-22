# Session events

Each Session writes one `events.jsonl` journal. Every row has a Session-local increasing `id`, a `kind`, and a timestamp `ts`. Cursors point to committed history; correlate work across Agents by combining Agent address, Turn, and request or tool-call ID.

## Reading events

```bash
kuku events SESSION_ID --limit 20 --json
```

Outputs one JSON record per line to `stdout` and prints the next-page flag/cursor to `stderr`. Use `--before` for older records, `--after` to scan forward, `--agent` / `--kind` to filter, or `--file <path> --all --json` to export a local journal (including sealed history). [CLI](cli.md#kuku-events) · [Server API](server-api.md)

## Record kinds

| Kind or group | What it records |
|---|---|
| `session.created`, `conversation.opened` | Session metadata and the opening of an Agent's conversation stream. |
| `runtime.operation` | Durable state transitions keyed by `record.operation`: `session_claimed`, `agent_created`, `agent_configured`, `input_delivered`, `inputs_accepted`, `input_withdrawal`, `turn_settled`, `execution_started`, `agent_stopped`, `agent_resumed`, plus pending requests and replies. |
| `turn.started`, `message.user`, `model.response`, `model.error` | Turn inputs and provider outcomes (`job_delivery: true` on `message.user` marks background Job feedback). `turn.completed`, `turn.interrupted`, and `turn.cancelled` mark terminal states. |
| `runtime.context_segment`, `runtime.context_admitted`, `runtime.step_prepared` | Frozen static context, newly admitted messages, and prepared Step bounds (`body_hash` identifies the encoded request). |
| `runtime.provider_report`, `runtime.model_usage`, `runtime.provider_replay` | Provider metadata, token counters, and protocol-native continuation payloads when available. |
| `runtime.model_retry` | `waiting`, `started`, and `stopped` retry phases for a prepared request (see payload schema below). |
| `model.recovery` | One-time output-limit recovery retry for a direct `Query` or child-Agent query (`attempt`, `max_attempts`, `reason`, `from_request_id`, `to_request_id`, and recovery-notice provenance). |
| `tool.call`, `runtime.tool_prepared`, `runtime.tool_dispatched`, `tool.result` | Requested tool call, bound args/definition, pre-side-effect dispatch, and recorded result (denied calls have a result without a dispatch). |
| `permission.requested`, `permission.allow`, `permission.deny` | Deferred authorization request and decision for a tool call; resumable wait/reply state lives in `runtime.operation`. |
| `handoff` | Compaction summary boundary used to open a successor context segment. |

Field naming note: execution events may put the Agent address in `conversation`, prepared Steps and provider reports use `agent`, and Session operations place the target inside `record`. For current state, read an Agent snapshot instead of folding events manually. [Session records](../../../crates/kuku/src/session/runtime/record.rs) · [Session SDK](sdk.md)

## `runtime.model_retry` payload

`runtime.model_retry` is stored as an `EventPayload::Unknown` JSON object:

| Field | Type | Meaning |
|---|---|---|
| `kind` | string | Always `runtime.model_retry`. |
| `agent`, `request_id` | string | Agent address and prepared request ID. |
| `turn` | integer | Owning Turn. |
| `failed_kind` | enum | `authentication`, `permission_denied`, `quota_exhausted`, `context_too_large`, `invalid_request`, `rate_limited`, `timeout`, `provider_unavailable`, `transport`, `internal`, `unknown`, or `invalid_response`. |
| `phase` | enum (optional for legacy rows) | `waiting` (logged before sleep), `started` (before redispatch), or `stopped`. Missing legacy `phase` means `started`. |
| `failed_attempt`, `max_attempts`, `attempt` | integer | One-based counters. `attempt` is the upcoming attempt in `waiting`/`started` and the final failed attempt in `stopped`; `failed_attempt` is the prior failed call when retrying and the current failed call when stopped. |
| `delay_ms` | integer | Backoff wait in `waiting`; `0` in `started` and `stopped`. |
| `will_retry` | boolean | `true` for `waiting`/`started`, `false` for `stopped`. |
| `message` | string | Bounded, sanitized error text (no credentials or terminal escape codes). |
| `status` | HTTP `u16` or null | HTTP status when known. |
| `stop_reason` | enum or null | `non_retryable`, `attempt_limit`, `request_budget`, `disabled`, or `stream_started`; `null` before stop. |
| `ts` | timestamp string | Event timestamp. |

Attempt counters include `started` rows (and legacy rows without `phase`), not `waiting` or `stopped`.

## Live deltas vs. committed records

Text deltas and progress notices are transient; the journal holds settled replies, tool results, and retained output references. After reconnecting, resume from your last committed cursor. A `runtime.tool_dispatched` without a matching `tool.result` means the action may already have modified the workspace. [Sessions](../how-it-works/sessions.md) · [File layout](file-layout.md)
