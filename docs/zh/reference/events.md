# Session 事件

每个 Session 维护一份 `events.jsonl` 日志。每条记录都有 Session 内单调递增的 `id`、事件类型 `kind` 和时间戳 `ts`。游标标识已提交的历史位置；跨 Agent 排查时结合 Agent 地址、Turn 编号以及请求或工具调用 ID 关联。

## 读取事件

```bash
kuku events SESSION_ID --limit 20 --json
```

每行输出一条完整 JSON 到 `stdout`，翻页参数与游标写到 `stderr`。用 `--before` 往旧记录翻，`--after` 往新记录读，`--agent` / `--kind` 过滤，或用 `--file <path> --all --json` 导出本地日志（含已封存历史）。[CLI](cli.md#kuku-events) · [HTTP 服务 API](server-api.md)

## 记录类型

| 类型或分组 | 记录内容 |
|---|---|
| `session.created` | Session 元数据。 |
| `runtime.operation` | 按 `record.operation` 区分的持久状态变更：`session_claimed`、`agent_created`、`agent_configured`、`input_delivered`、`inputs_accepted`、`input_withdrawal`、`turn_settled`、`execution_started`、`agent_stopped`、`agent_resumed`，以及待决请求与回复。 |
| `turn.started`、`message.user`、`model.response`、`model.error` | Turn 输入与模型结果（带 `job_delivery: true` 的 `message.user` 表示后台 Job 反馈）。`turn.completed`、`turn.interrupted`、`turn.cancelled` 表示终态。 |
| `runtime.context_segment`、`runtime.context_admitted`、`runtime.step_prepared` | 冻结的静态上下文、新接纳消息与确认边界的 Step 准备记录（`body_hash` 标识编码后的请求）。 |
| `runtime.checkpoint_file_changes` | 压缩检查点新增的一批文件变动记录，通过 `segment` 关联检查点。模型收到有界预览。 |
| `runtime.provider_report`、`runtime.model_usage`、`runtime.provider_replay` | Provider 元数据、token 计数及协议原生续接载荷（若可用）。 |
| `runtime.model_retry` | 已准备请求的 `waiting`、`started`、`stopped` 重试阶段（载荷字段见下表）。 |
| `model.recovery` | 直接 `Query` 或子 Agent 查询遇到输出上限时的一次恢复重试（含 `attempt`、`max_attempts`、`reason`、`from_request_id`、`to_request_id` 及恢复提示来源）。 |
| `tool.call`、`runtime.tool_prepared`、`runtime.tool_dispatched`、`tool.result` | 调用请求、绑定后的参数/定义、副作用前派发与落盘结果（被拒调用有结果无派发）。 |
| `permission.requested`、`permission.allow`、`permission.deny` | 单次工具调用的延后授权请求与决定；可恢复的等待与回复记在 `runtime.operation` 中。 |
| `handoff` | 用于开启后继上下文段的压缩摘要边界。 |

字段位置说明：执行事件可能用 `conversation` 表示 Agent 地址，Step 准备与供应商回报用 `agent`，Session 操作把目标放在 `record` 里。查询当前状态建议直接读 Agent 快照，不必手工折叠事件。[Session 记录定义](../../../crates/kuku/src/session/runtime/record.rs) · [Session SDK](sdk.md)

## `runtime.model_retry` 载荷

`runtime.model_retry` 以 `EventPayload::Unknown` JSON 对象存储：

| 字段 | 类型 | 含义 |
|---|---|---|
| `kind` | 字符串 | 固定为 `runtime.model_retry`。 |
| `agent`、`request_id` | 字符串 | Agent 地址与已准备请求 ID。 |
| `turn` | 整数 | 所属 Turn。 |
| `failed_kind` | 枚举 | `authentication`、`permission_denied`、`quota_exhausted`、`context_too_large`、`invalid_request`、`rate_limited`、`timeout`、`provider_unavailable`、`transport`、`internal`、`unknown` 或 `invalid_response`。 |
| `phase` | 枚举 | `waiting`（休眠前写入）、`started`（重派发前写入）或 `stopped`。 |
| `failed_attempt`、`max_attempts`、`attempt` | 整数 | 从 1 开始的计数。`waiting`/`started` 中 `attempt` 为即将进行的尝试，`stopped` 中为最终失败的尝试；`failed_attempt` 重试时指前一次失败调用，停止时指当前失败调用。 |
| `delay_ms` | 整数 | `waiting` 中的实际退避时长；`started` 和 `stopped` 为 `0`。 |
| `will_retry` | 布尔值 | `waiting`/`started` 为 `true`，`stopped` 为 `false`。 |
| `message` | 字符串 | 有界且已清理的失败说明（不含凭据或终端控制符）。 |
| `status` | HTTP `u16` 或 null | 已知时为 HTTP 状态码。 |
| `stop_reason` | 枚举或 null | `non_retryable`、`attempt_limit`、`request_budget`、`disabled` 或 `stream_started`；停止前为 `null`。 |
| `ts` | 时间戳字符串 | 事件时间戳。 |

尝试次数统计只计入 `started` 行，`waiting` 和 `stopped` 不增加尝试次数。

## 实时增量与已提交记录

流式文本和进度通知都是临时的；日志保存最终回复、工具结果与输出引用。断线重连后从上次已提交游标续读即可。如果看到 `runtime.tool_dispatched` 但没有对应的 `tool.result`，说明该动作可能已经改过工作区。[Session 机制](../how-it-works/sessions.md) · [文件布局](file-layout.md)
