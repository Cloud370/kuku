# 排查 CLI 运行

## 查看 Agent 快照与事件

在同一工作区里把 `SESSION_ID` 和 `reviewer` 替换成实际值：

```bash
kuku list SESSION_ID
kuku show SESSION_ID --agent reviewer --json
kuku events SESSION_ID --kind runtime.step_prepared --limit 20 --json
```

`kuku show --json` 返回该 Agent 已落盘的状态、最近回复、累计用量以及待决问题/授权（`config` 只含该 Agent 的显式覆盖项，沿用默认值的字段为 `null`）。`kuku events` 会把下一页游标打印到 `stderr`。排查时先看 `runtime.step_prepared` 确认请求实际命中的连接、模型和参数，再按 Agent、Turn 和请求/调用 ID 对照 `model.response`、`tool.call` 与 `tool.result`。[事件参考](../reference/events.md)

## 核对模型用量

```bash
kuku events SESSION_ID --kind runtime.model_usage --json |
  jq -c '{agent, turn, request_id, usage}'
```

Provider 报告缓存读取时，`input_tokens` 会包含这部分计数；字段缺失代表端点没返回。要排查跨轮复用情况，继续同一 Agent，对比 `step_prepared` 的上下文段和回报的缓存命中数即可。[SDK 限额与用量](../reference/sdk.md#限额与用量)

## 抓取完整 Provider 请求

预发布构建默认带协议诊断，先看事件通常就够了。如果确实需要完整请求体和 SSE 帧，等受管本地服务空闲后打开 trace：

```bash
KUKU_PROVIDER_TRACE=1 kuku server restart
kuku run --no-skills "Reply with one sentence"
```

trace 文件写在 `$KUKU_HOME/logs/provider-trace/`。含密钥的请求头会自动脱敏，但正文里仍有提示词、文件片段和工具输出，对外分享前记得清理。[环境变量](../reference/environment-variables.md)
