# Debug a CLI run

## Identify the Agent and its last outcome

In the same workspace, replace `SESSION_ID` with the printed ID and `reviewer` with the Agent address you want to inspect.

```bash
kuku list SESSION_ID
kuku show SESSION_ID --agent reviewer --json
kuku events SESSION_ID --after 0 --limit 100 --json
```

The snapshot contains the recorded status, last response, usage and pending question or permission. Its `config` fields are Agent overrides and may be null when defaults apply. The event command prints a cursor to stderr for the next page. Locate `runtime.step_prepared` to see the effective connection, model and settings before a request. Then correlate `model.response`, `tool.call` and `tool.result` by Agent, Turn and request or call ID. [Events](../reference/events.md)

## Check reported usage

For one event page, examine the counters returned by the endpoint.

```bash
kuku events SESSION_ID --limit 256 --json |
  jq -c 'select(.kind == "runtime.model_usage") | {agent, turn, request_id, usage}'
```

`input_tokens` includes cache reads when reported. A missing counter means the endpoint supplied no value. To examine reuse across Turns, continue the same Agent and compare its prepared segment and provider-reported usage. [SDK accounting](../reference/sdk.md#limits-and-accounting)

## Inspect the provider request

Enable tracing on a small run when the recorded settings alone cannot explain a protocol or routing issue.

```bash
KUKU_PROVIDER_TRACE=1 kuku run --no-skills "Reply with one sentence"
```

Trace files are written under `$KUKU_HOME/logs/provider-trace/<yyyy-mm-dd>/<session-id>.jsonl`. They include request bodies and streamed responses. Headers containing secrets are redacted, but prompts, file contents and tool output may still be present. Share the relevant event IDs and fields. [Environment variables](../reference/environment-variables.md)
