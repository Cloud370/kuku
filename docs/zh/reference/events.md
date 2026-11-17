# Session 事件

每个 Session 维护一份 `events.jsonl` 日志。每条记录都含有在 Session 内单调递增的 `id`、事件种类 `kind` 和时间戳 `ts`。游标标识已提交的历史位置。跨 Agent 对照执行时，同时使用 Agent 地址、Turn 编号及相应的请求或工具调用 ID。

## 读取一页

```bash
kuku events SESSION_ID --after 0 --limit 100 --json
```

CLI 在标准输出中逐行写出 JSON，在标准错误中给出下一游标。将该游标传给 `--after` 读取下一页。加上 `--agent reviewer` 可以过滤当前页。即使过滤后这一页为空，游标也按实际扫描过的日志前移。[CLI](cli.md#kuku-events) · [Server API](server-api.md)

## 公开 Session SDK 记录的事实

| 事件或事件组 | 记录内容 |
|---|---|
| `session.created`、`conversation.opened` | Session 元数据，以及打开某个 Agent 的对话流。 |
| `runtime.operation` | 状态变化由 `record.operation` 指定。例如 `session_claimed`、`agent_created`、`agent_configured`、`input_delivered`、`inputs_accepted`、`input_withdrawal` 和 `turn_settled`。`execution_started`、`agent_stopped` 和 `agent_resumed` 记录定向执行控制。待决请求和关联回复也通过 Session 操作记录。 |
| `turn.started`、`message.user`、`model.response`、`model.error` | Turn 的输入和模型服务结果。`turn.completed`、`turn.interrupted`、`turn.cancelled` 记录终止状态。 |
| `runtime.context_segment`、`runtime.context_admitted`、`runtime.step_prepared` | 静态上下文、新接纳的消息，以及确认消息边界的 Step 准备记录。`body_hash` 标识编码后的请求。 |
| `runtime.provider_report`、`runtime.model_usage`、`runtime.provider_replay` | 模型服务实际回报的元数据、token 用量或可用的协议原生续接数据。 |
| `runtime.model_retry` | 记录对已准备请求的有界重试及首次失败类型。 |
| `tool.call`、`runtime.tool_prepared`、`runtime.tool_dispatched`、`tool.result` | 调用请求、最终参数和工具定义、副作用前的派发记录、已落盘的结果。被拒绝的动作可以有工具结果而没有派发记录。 |
| `permission.requested`、`permission.allow`、`permission.deny` | 某次工具调用的延后授权或决定。可恢复的 Agent 等待和关联回复记录在 `runtime.operation`。 |
| `handoff` | 用于准备后继上下文段的摘要边界。 |

执行事件可能用 `conversation` 表示 Agent 地址。准备记录和供应商回报用 `agent`。Session 操作把目标存在 `record` 内。需要当前状态时可直接读取 SDK 的 Agent 快照。[Session 记录定义](../../../crates/kuku/src/session/runtime/record.rs) · [Session SDK](sdk.md)

## 实时输出

文本增量和进度通知可以是临时的。日志保留最终响应、工具结果和输出引用。重连后从上次游标读取已提交的记录。已派发但结果尚未确认的工具可能已经改变了工作目录。[Session](../how-it-works/sessions.md) · [文件布局](file-layout.md)
