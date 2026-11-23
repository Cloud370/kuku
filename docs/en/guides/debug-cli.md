# Debug CLI runs

## Inspect the Agent snapshot and events

From the same workspace, replace `SESSION_ID` and the Agent address (`reviewer`):

```bash
kuku list SESSION_ID
kuku show SESSION_ID --agent reviewer --json
kuku events SESSION_ID --kind runtime.step_prepared --limit 20 --json
```

`kuku show --json` returns the Agent's durable status, latest reply, usage totals, and any pending question or permission (`config` shows explicit overrides for that Agent, so inherited defaults appear as `null`). `kuku events` prints the next-page cursor to `stderr`. Start with `runtime.step_prepared` to verify the resolved connection, model, and settings for a request, then correlate `model.response`, `tool.call`, and `tool.result` by Agent, Turn, and request/call ID. [Events](../reference/events.md)

## Check provider-reported usage

```bash
kuku events SESSION_ID --kind runtime.model_usage --json |
  jq -c '{agent, turn, request_id, usage}'
```

`input_tokens` includes cache reads when the endpoint reports them; a missing field means the provider returned no value. To check prompt reuse across Turns, continue the same Agent and compare prepared segments against reported cache hits. [SDK accounting](../reference/sdk.md#limits-and-accounting)

## Capture raw provider requests

Check protocol diagnostics in preview builds first. If you need full request bodies and SSE frames, enable provider tracing while the managed local service is idle:

```bash
KUKU_PROVIDER_TRACE=1 kuku server restart
kuku run --no-skills "Reply with one sentence"
```

Trace files land in `$KUKU_HOME/logs/provider-trace/`. Secret headers are redacted, but request bodies still contain prompts, file snippets, and tool outputs — scrub before sharing. [Environment variables](../reference/environment-variables.md)
