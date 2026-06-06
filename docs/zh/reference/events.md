# Events

`events.jsonl` 是追加写入的 Session 事实日志。每一行都是一个 Session 的已持久化事实。

## 命名规则

```text
<domain>.<action>
```

全部使用小写，并以点分隔。

## 事件类型

| Event | Meaning |
|---|---|
| `session.meta` | Session 元数据。新 Session 中的第一个事件。 |
| `context.prelude` | 用于上下文重建的运行时 prelude。 |
| `context.sources` | 上下文来源摘要。 |
| `turn.start` | 一轮开始。 |
| `user.input` | 本轮的用户 Prompt。 |
| `model.response` | 已完成的 provider 响应。 |
| `model.error` | provider 失败。 |
| `tool.call` | 一次请求的 Tool 调用。 |
| `permission.requested` | 一次 Tool 调用的持久待处理权限状态。 |
| `permission.allow` | Tool 授权允许决策。 |
| `permission.deny` | Tool 授权拒绝决策。 |
| `tool.result` | 一次 Tool 调用的结果。 |
| `handoff` | handoff 摘要载荷。 |
| `turn.end` | 一轮结束。 |
| `turn.rollback` | 回滚标记。 |
| `turn.rollback.undo` | 撤销一次回滚。 |

## 通用字段

每一行已持久化事件至少包含：

| Field | Meaning |
|---|---|
| `id` | Session 内单调递增的整数 |
| `type` | 事件类型 |
| `ts` | ISO 8601 时间戳 |
| `turn` | 轮作用域事件的轮编号 |

## 回滚作用域值

`turn.rollback` 会记录以下作用域值之一：

- `conversation_only`
- `files_only`
- `both`

## 权限状态

`permission.requested` 记录某个 Tool 调用正在等待 Host 授权。它是持久的待处理权限状态，不是允许或拒绝决策，也不是可观测性日志记录。

当 Host 解决该请求后，kuku 会在 `tool.result` 之前追加写入 `permission.allow` 或 `permission.deny`。

## Session 事实与运行时流

并非每个运行时事件都是 Session 事实。流式增量，例如文本块、thinking 块、实时 Tool 输出，以及 host 可见的日志记录，都是运行时流事件，不会写入 `events.jsonl`。

HTTP 线路事件见 [Server API](server-api.md)。
