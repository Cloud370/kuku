# Agent Loop

Nothing happened until it is in `events.jsonl`. The loop rebuilds context from files before every model call.

```text
turn.start
  → user.input
  → model.request
  → model.response
      stop_reason = tool_use ?
        yes → tool.call → permission.request → permission.decision → tool.result → loop to model.request
        no  → turn.end
```

## Per turn

1. Append `turn.start` and `user.input`.
2. Rebuild `messages[]`. See [architecture.md](architecture.md#context-assembly-a2b) for the full assembly order.
3. Append `model.request` with resolved provider, model, params, and provenance.
4. Call model, stream text to host. On completion, append `model.response`.
5. If `end_turn`: append `turn.end`, stop.
6. If `tool_use`: collect all tool calls, append all `tool.call`, run permission gate, execute, append all `tool.result` in original order, loop to step 2.

## Response group

A `model.response` and its immediately following `tool.call[]` events form a response group. During context rebuild, they become one assistant message with `tool_use` blocks. This is the stable recovery unit.

## Tool execution

All tool calls from one `model.response` run in parallel. The model controls
ordering: dependent operations go in separate turns, independent operations
in the same turn. Results are always appended in the model's original
`tool.call` order.

Tool results go into `events.jsonl` first. The next context rebuild reads them as user `tool_result` blocks.

## Errors

| Scenario | Event |
|----------|-------|
| Provider auth, rate limit, network, overflow | `model.error` |
| Invalid tool arguments | `tool.result {status:"error"}` |
| Permission denied | `permission.decision deny` + `tool.result {status:"blocked"}` |
| User cancels tool | `tool.result {status:"cancelled"}` |

`model.error` is diagnostic — it does not become a model message. Every `tool.call` must have a paired `tool.result`.

## Crashes

Only appended events are trusted.

| Crash after | Recovery sees |
|-------------|---------------|
| `user.input` | A turn was started |
| `model.request` | A request was sent, no confirmed response |
| `tool.call` | A tool was requested, no confirmed result |

Missing `tool.result` events are backfilled as `status:"cancelled"` on resume. Half-finished model responses are not guessed.

## Cancellation

`Run::cancel()` stops the current operation. The cancelled `model.response`
enters history, so the model sees what was produced before interruption.

| State | Behaviour |
|-------|-----------|
| Streaming | Abort via `tokio::select!`, write `model.response` with `stop_reason:"cancelled"` and truncated text |
| Waiting for permission | Deny all pending, write `tool.result {status:"blocked"}`, then `turn.end` |
| Executing tools | Running tools write `cancelled`; completed tools kept; `turn.end` |
| Idle | Direct `turn.end` |
