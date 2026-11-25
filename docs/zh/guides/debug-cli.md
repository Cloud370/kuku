# 排查 CLI 运行

## 核对运行所用的配置

```bash
kuku config validate
kuku server status
kuku config show --json
```

将配置结果中的 `path` 和 `service.state` 与运行中的服务对照。文件校验为 `valid` 时，仍可能有待重载，或与服务使用的路径不同。沿用任务的 `--data-dir`，显式指定过文件时也沿用 `--config`。[CLI 配置命令](../reference/cli.md#kuku-config)

## 查看 Agent 快照与事件

在同一工作区里把 `SESSION_ID` 和 `reviewer` 替换成实际值：

```bash
kuku list SESSION_ID
kuku show SESSION_ID --agent reviewer --json
kuku events SESSION_ID --kind runtime.step_prepared --limit 20 --json
```

`kuku run --json` 和 `--stream-json` 的末行给出运行结果。拿到 Session ID 后，可查 Agent 的持久快照，包括最近回复、累计用量和待决问题或授权。其中的 `config` 描述 Agent 请求的覆盖设置。先通过 `runtime.step_prepared` 核对实际连接、精确模型 ID 和请求设置，再按 Agent、Turn 和请求或调用 ID 对照 `model.response`、`tool.call` 与 `tool.result`。`kuku events` 将翻页游标写往 stderr。[事件参考](../reference/events.md)

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
