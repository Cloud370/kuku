# 排查 CLI 运行

## 找到 Agent 和最近一次结果

在同一工作目录中，将 `SESSION_ID` 替换为打印出的标识，将 `reviewer` 替换为要检查的 Agent 地址。

```bash
kuku list SESSION_ID
kuku show SESSION_ID --agent reviewer --json
kuku events SESSION_ID --after 0 --limit 100 --json
```

快照包含已落盘的状态、最近回复、用量及待决问题或授权。`config` 记录该 Agent 显式指定的配置，沿用默认值的字段可以是 `null`。事件命令会在标准错误中打印下一页的游标。先找 `runtime.step_prepared` 核对请求实际选择的连接、模型和设置，再按 Agent、Turn 以及请求或调用 ID 对照 `model.response`、`tool.call` 和 `tool.result`。[事件参考](../reference/events.md)

## 查看实际用量

可以在一页事件中提取端点报告的计数。

```bash
kuku events SESSION_ID --limit 256 --json |
  jq -c 'select(.kind == "runtime.model_usage") | {agent, turn, request_id, usage}'
```

`input_tokens` 包含端点报告的缓存读取。字段缺失表示端点没有提供相应数值。检查跨轮次复用时，继续使用同一 Agent，对照它的上下文段和模型服务回报的用量。[SDK 用量](../reference/sdk.md#限额与用量)

## 查看模型服务请求

当已落盘的设置不足以解释协议或路由问题时，可以在服务上启用 trace。等受管本地服务空闲后再执行以下命令。

```bash
KUKU_PROVIDER_TRACE=1 kuku server restart
kuku run --no-skills "Reply with one sentence"
```

trace 写入 `$KUKU_HOME/logs/provider-trace/<yyyy-mm-dd>/<session-id>.jsonl`，包含请求正文和响应流。敏感请求头会脱敏，但正文仍可能含有提示、文件片段和工具输出。对外说明问题时可以提供相关事件 ID 和字段。[环境变量](../reference/environment-variables.md)
