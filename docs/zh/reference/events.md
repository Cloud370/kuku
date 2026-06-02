# Events

`events.jsonl` 是追加写入的日志。每一行都是一个已持久化事件。

## 命名规则

```text
<domain>.<action>
```

全部使用小写，并以点分隔。

## 事件类型

| Event | Meaning |
|---|---|
| `session.meta` | Session 元数据。新 Session 中的第一个事件。 |
| `policy.loaded` | 已加载 `policy.md` 的哈希。可选。 |
| `turn.start` | 一轮开始。 |
| `user.input` | 本轮的用户 Prompt。 |
| `model.request` | 已解析的 provider 请求元数据。 |
| `model.response` | 已完成的 provider 响应。 |
| `model.error` | provider 失败。 |
| `tool.call` | 一次请求的 Tool 调用。 |
| `tool.result` | 一次 Tool 调用的结果。 |
| `permission.request` | 等待授权的 Tool。 |
| `permission.decision` | 授权结果。 |
| `turn.end` | 一轮结束。 |
| `turn.rollback` | 回滚标记。 |
| `turn.rollback.undo` | 撤销一次回滚。 |
| `handoff.trigger` | 上下文 handoff 触发器。 |
| `handoff` | handoff 摘要载荷。 |
| `plugin.hook` | hook 执行结果。 |

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

- `ConversationOnly`
- `FilesOnly`
- `Both`

## 仅运行时与已持久化

并非每个运行时事件都会被持久化。流式增量，例如文本块、thinking 块和实时 Tool 输出，都是面向 host 的运行时事件，不会写入 `events.jsonl`。

HTTP 线路事件见 [Server API](server-api.md)。
