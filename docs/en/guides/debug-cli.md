# Debug CLI Runs

Use this guide when a run behaves unexpectedly and you need evidence about the model request, tool loop, cache usage, or agent delegation.

## Start With Runtime Logs

Most debugging should start with the runtime log. It records provider requests, usage, tool rounds, and errors without storing full provider request bodies.

```bash
KUKU_HOME=${KUKU_HOME:-$HOME/.kuku}
jq -r 'select(.kind=="runtime.model_usage") |
  [.turn,.request_id,.data.input_tokens,.data.cache_read_input_tokens,
   .data.cache_creation_input_tokens,.data.input_tokens_total,.data.cache_hit_rate] |
  @tsv' "$KUKU_HOME/logs/runtime/$(date +%F).jsonl"
```

Interpretation:

- `cache_read_input_tokens` shows how many input tokens came from provider cache.
- `cache_creation_input_tokens` shows how many input tokens were written to provider cache.
- `cache_hit_rate` is `cache_read_input_tokens / input_tokens_total` for that provider request.
- `request_id` is scoped to a conversation turn; use `events.jsonl` to map it back to tools and conversations.

## Turn On Provider Trace Only When Needed

Provider trace records the provider-facing request and response stream. Enable it only for short diagnostic runs.

```bash
KUKU_PROVIDER_TRACE=1 kuku run --model anthropic_cache --no-skills \
  "Use the agent tool once and summarize the evidence."
```

Trace files are written under:

```text
$KUKU_HOME/logs/provider-trace/<yyyy-mm-dd>/<session-id>.jsonl
```

Headers that can contain secrets are redacted, but request bodies can still contain prompts, file snippets, and tool results. Do not paste complete trace files into issues or chat. Share only focused summaries.

## Confirm The Actual Provider Request

Use provider trace to verify the real model, URL, and cache-control shape.

```bash
jq -r 'select(.direction=="request") | .body as $body | {
  turn,
  request_id,
  url,
  trace_model: .model,
  body_model: $body.model,
  top_cache: ($body.cache_control // null),
  system_type: ($body.system | type),
  system_marker_count: [($body.system // [])[]? | select(.cache_control != null)] | length,
  tool_marker_count: [($body.tools // [])[]? | select(.cache_control != null)] | length,
  message_marker_count: [($body.messages // []) as $messages |
    range(0; ($messages|length)) as $i |
    ($messages[$i].content // []) as $content |
    range(0; ($content|length)) as $j |
    select($content[$j].cache_control != null)] | length
} | @json' "$KUKU_HOME/logs/provider-trace/<date>/<session-id>.jsonl"
```

For Anthropic-format providers, expected cache-control diagnostics are:

- `top_cache` is `{"type": "ephemeral"}`; kuku uses top-level automatic caching.
- `system_marker_count`, `tool_marker_count`, and `message_marker_count` are 0.
- No per-block cache markers; the single top-level `cache_control` triggers Anthropic automatic caching.

## Debug Agent Reuse

When checking whether a delegated agent reused context, run both the parent and the agent on the intended tier and reuse the same agent conversation address.

```bash
KUKU_PROVIDER_TRACE=1 kuku run --model anthropic_cache --no-skills \
  "Use agent(to=cacheprobe/official, tier=anthropic_cache) to inspect CACHE_CONTROL_SENTINEL."

KUKU_PROVIDER_TRACE=1 kuku run --model anthropic_cache --no-skills \
  --session <session-id> \
  "Use the same agent conversation cacheprobe/official again. Do not pass tier this time."
```

Then map events back to conversations:

```bash
jq -r 'select(.kind=="turn.started" or .kind=="message.user" or
  .kind=="context.sources" or .kind=="model.response" or
  .kind=="tool.call" or .kind=="tool.result" or .kind=="turn.completed") |
  [.id,.kind,(.conversation // ""),(.turn // ""),(.request_id // ""),(.tool // ""),(.summary // "")] |
  @tsv' "$KUKU_HOME/p/<workspace-key>/sessions/<session-id>/events.jsonl"
```

Expected signs of reuse:

- Later provider requests in the same turn usually have high `cache_read_input_tokens`.
- Follow-up turns in the same agent conversation should reuse stable prompt, tool, and prior result prefixes.
- If the provider supports explicit Anthropic prompt caching, first requests may show `cache_creation_input_tokens`, and later requests may show `cache_read_input_tokens`.

## Keep Debug Runs Safe

- Use `--no-skills` or `--no-agents` when those surfaces are not part of the bug.
- Use a temporary `KUKU_HOME` for experiments that should not pollute real sessions.
- Keep prompts small when provider trace is enabled.
- Share summarized `jq` output instead of full request bodies.
- Never paste API keys; provider trace redacts headers, but config files and shell history are outside trace redaction.
