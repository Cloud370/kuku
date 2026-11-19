# Session events

Each Session writes one `events.jsonl` journal. Stored rows have a Session-local increasing `id`, a `kind` and a timestamp `ts`. A cursor identifies committed history. Use the Agent address, Turn and request or tool-call ID when correlating work across Agents.

## Reading a page

```bash
kuku events SESSION_ID --after 0 --limit 100 --json
```

The CLI writes one JSON record per line to stdout and the next cursor to stderr. Pass that cursor to `--after` to read the next page. Add `--agent reviewer` to filter the current page. The cursor advances across all scanned records even if the filter produced an empty page. [CLI](cli.md#kuku-events) · [Server API](server-api.md)

## Facts recorded by the public Session SDK

| Kind or group | What it records |
|---|---|
| `session.created`, `conversation.opened` | Session metadata and the opening of an Agent's conversation stream. |
| `runtime.operation` | `record.operation` selects the state change. Examples include `session_claimed`, `agent_created`, `agent_configured`, `input_delivered`, `inputs_accepted`, `input_withdrawal` and `turn_settled`. `execution_started`, `agent_stopped` and `agent_resumed` track targeted execution control. Pending requests and their replies are also Session operations. |
| `turn.started`, `message.user`, `model.response`, `model.error` | Input and provider outcomes for a Turn. `turn.completed`, `turn.interrupted` and `turn.cancelled` describe terminal outcomes. |
| `runtime.context_segment`, `runtime.context_admitted`, `runtime.step_prepared` | Static context, newly admitted messages and the prepared Step confirming their bounds. `body_hash` identifies the encoded request. |
| `runtime.provider_report`, `runtime.model_usage`, `runtime.provider_replay` | Provider-reported metadata, token counters or protocol-native continuation data when available. |
| `runtime.model_retry` | The interactive-client projection accepts this extended retry-decision shape for a prepared request; the core producer update is not part of this branch, so current runtime events still use the legacy shape. Missing legacy `phase` means `started`. In the extended shape, `phase` is `waiting`, `started` or `stopped`; `attempt` is the next one-based attempt while waiting or starting, and the final failed attempt when stopped. `failed_attempt`, `max_attempts`, `failed_kind`, `request_id`, `delay_ms`, `ts`, `will_retry`, `message`, `status` and `stop_reason` are optional policy or diagnostic fields. Only `will_retry` states whether another request will be sent. `delay_ms` applies only to `waiting`; `stop_reason` can be `non_retryable`, `attempt_limit`, `request_budget`, `disabled` or `stream_started`. |
| `model.recovery` | A model-output recovery with its original `attempt`, `max_attempts`, `reason`, `from_request_id` and `to_request_id`, plus recovery-specific diagnostic details. |
| `tool.call`, `runtime.tool_prepared`, `runtime.tool_dispatched`, `tool.result` | The request, its final arguments and definition, dispatch before the side effect, and the recorded outcome. A denied action can have a result without a dispatch. |
| `permission.requested`, `permission.allow`, `permission.deny` | A deferred authorization or a decision about a particular tool call. `runtime.operation` carries the resumable Agent wait and correlated reply. |
| `handoff` | A summary boundary used to prepare a successor context segment. |

Execution events may use `conversation` for the Agent address. Prepared Steps and provider reports use `agent`. Session operations carry the target inside `record`. Use the SDK's Agent snapshot when the needed result is current state. [Session records](../../../crates/kuku/src/session/runtime/record.rs) · [Session SDK](sdk.md)

## Live output

Text deltas and progress notifications can be transient. The journal records the settled response, tool results and retained output references. After reconnecting, read committed rows from the last cursor. A dispatched tool with no confirmed result may already have affected the workspace. [Sessions](../how-it-works/sessions.md) · [File layout](file-layout.md)
