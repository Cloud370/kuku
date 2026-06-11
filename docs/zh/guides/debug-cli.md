# Debug CLI Runs

当一次运行行为不符合预期，并且你需要确认模型请求、Tool loop、缓存用量或 Agent 委托时，使用这份指南。

## 先看 Runtime Logs

大多数调试都应该从 runtime log 开始。它记录 provider request、usage、tool round 和错误，但不会保存完整 provider request body。

```bash
KUKU_HOME=${KUKU_HOME:-$HOME/.kuku}
jq -r 'select(.kind=="runtime.model_usage") |
  [.turn,.request_id,.data.input_tokens,.data.cache_read_input_tokens,
   .data.cache_creation_input_tokens,.data.input_tokens_total,.data.cache_hit_rate] |
  @tsv' "$KUKU_HOME/logs/runtime/$(date +%F).jsonl"
```

字段含义：

- `cache_read_input_tokens` 表示有多少 input tokens 来自 provider cache。
- `cache_creation_input_tokens` 表示有多少 input tokens 被写入 provider cache。
- `cache_hit_rate` 是该 provider request 的 `cache_read_input_tokens / input_tokens_total`。
- `request_id` 作用域在 conversation turn 内；用 `events.jsonl` 把它映射回 tool 和 conversation。

## 只有需要时才开启 Provider Trace

Provider trace 会记录面向 provider 的请求和响应流。只在短时间诊断时开启。

```bash
KUKU_PROVIDER_TRACE=1 kuku run --model anthropic_cache --no-skills \
  "Use the agent tool once and summarize the evidence."
```

Trace 文件位置：

```text
$KUKU_HOME/logs/provider-trace/<yyyy-mm-dd>/<session-id>.jsonl
```

可能包含 secret 的 headers 会被打码，但 request body 仍可能包含 prompt、文件片段和 tool results。不要把完整 trace 文件粘贴到 issue 或聊天里，只分享聚焦后的摘要。

## 确认实际 Provider Request

用 provider trace 验证真实 model、URL 和 cache-control 形状。

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

对 Anthropic-format provider，预期的 cache-control 诊断结果是：

- `top_cache` 是 `null`；kuku 不发送顶层 `cache_control`。
- `system_type` 是 `array`。
- cache marker 出现在 provider content blocks 上，例如 system text block、tool schema 和 conversation content block。

## 调试 Agent 复用

检查 delegated agent 是否复用上下文时，让 parent 和 agent 都使用目标 tier，并复用同一个 agent conversation address。

```bash
KUKU_PROVIDER_TRACE=1 kuku run --model anthropic_cache --no-skills \
  "Use agent(to=cacheprobe/official, tier=anthropic_cache) to inspect CACHE_CONTROL_SENTINEL."

KUKU_PROVIDER_TRACE=1 kuku run --model anthropic_cache --no-skills \
  --session <session-id> \
  "Use the same agent conversation cacheprobe/official again. Do not pass tier this time."
```

再把 events 映射回 conversation：

```bash
jq -r 'select(.kind=="turn.started" or .kind=="message.user" or
  .kind=="context.sources" or .kind=="model.response" or
  .kind=="tool.call" or .kind=="tool.result" or .kind=="turn.completed") |
  [.id,.kind,(.conversation // ""),(.turn // ""),(.request_id // ""),(.tool // ""),(.summary // "")] |
  @tsv' "$KUKU_HOME/p/<workspace-key>/sessions/<session-id>/events.jsonl"
```

复用正常时通常会看到：

- 同一 turn 后续 provider requests 通常有较高 `cache_read_input_tokens`。
- 同一个 agent conversation 的后续 turns 应复用稳定 prompt、tools 和历史 tool results 前缀。
- 如果 provider 支持显式 Anthropic prompt caching，首次请求可能出现 `cache_creation_input_tokens`，后续请求可能出现 `cache_read_input_tokens`。

## 保持调试安全

- 如果 bug 与 Skills 或 Agents 无关，用 `--no-skills` 或 `--no-agents` 收窄变量。
- 对不想污染真实 sessions 的实验，使用临时 `KUKU_HOME`。
- 开启 provider trace 时保持 prompt 较小。
- 分享 `jq` 摘要，而不是完整 request body。
- 不要粘贴 API key；provider trace 会打码 headers，但 config 文件和 shell history 不属于 trace 打码范围。
