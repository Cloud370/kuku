# Debug CLI runs

## Check the config used by the run

```bash
kuku config validate
kuku server status
kuku config show --json
```

Compare the config result's `path` and `service.state` with the running service. A `valid` file can be awaiting reload or selected at a different path. Use the same `--data-dir` and, when explicit, `--config` as the task. [Config commands](../reference/cli.md#kuku-config)

## Inspect the Agent snapshot and events

From the same workspace, replace `SESSION_ID` and the Agent address (`reviewer`):

```bash
kuku list SESSION_ID
kuku show SESSION_ID --agent reviewer --json
kuku events SESSION_ID --kind runtime.step_prepared --limit 20 --json
```

`kuku run --json` and the final `--stream-json` line give the run outcome. Use the Session ID to query the durable Agent snapshot, including the latest reply, usage totals, and pending questions or permissions. Its `config` describes requested Agent overrides. Start with `runtime.step_prepared` to verify the resolved connection, exact model ID, and request settings, then correlate `model.response`, `tool.call`, and `tool.result` by Agent, Turn, and request or call ID. `kuku events` prints the next-page cursor to stderr. [Events](../reference/events.md)

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
