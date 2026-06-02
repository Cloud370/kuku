# Sessions

`Session` 是一个目录。kuku 不会把单独的 Session 对象或数据库记录作为事实来源。

## Layout

Session 位于 `$KUKU_HOME` 中按工作区划分的区域下：

```text
$KUKU_HOME/p/<workspace-path>/sessions/<id>/
|- lock
|- events.jsonl
|- pre-revert-<id>/
`- subs/
```

`events.jsonl` 是持久历史。子 Subagent Session 位于 `subs/` 下。

## Event log

`events.jsonl` 中每一行都是一个事件。常见事件类型包括：

- `session.meta`
- `turn.start`
- `user.input`
- `model.request`
- `model.response`
- `tool.call`
- `tool.result`
- `turn.end`

读取方信任文件顺序。恢复时会忽略末尾不完整的行。

## Lifecycle

### New session

在没有 Session id 的情况下启动运行，会创建新的 Session 目录，并在第一轮之前写入 `session.meta`。

### Continuing a session

在已有 Session id 的情况下启动运行，会向该 Session 追加新的一轮。kuku 会从事件日志重建先前上下文。

### Status

每个 Session 都处于以下三种状态之一：

| Status | Meaning |
|--------|---------|
| `Active` | 存在活动的写锁。 |
| `Done` | 不存在锁，且最后一个事件是 `turn.end`。 |
| `Interrupted` | 不存在锁，且最后一个事件不是 `turn.end`。 |

## Writer lock

同一时间只能有一个写入者向 Session 追加内容。读取操作可以并发进行。

## Handoff

当上下文使用量超过配置阈值时，kuku 会在模型调用前注入一条 handoff 指令。如果模型返回一个 `<kuku_handoff>` 文档，运行时会把它存入事件日志，并将其作为未来上下文重建的摘要边界。

下一次请求会保留少量最近轮次，并用 handoff 摘要替换更早的历史。

## Rollback

Rollback 只追加，不删除。kuku 会记录 rollback 标记事件，而不是删除历史。

存在三种范围：

| Scope | Effect |
|-------|--------|
| `ConversationOnly` | 让先前轮次不再参与未来的上下文重建。 |
| `FilesOnly` | 将工作区文件回退到更早的轮次。 |
| `Both` | 同时应用这两种行为。 |

文件回退会使用已经保存在 `tool.result` 数据中的快照，并将回退前备份存放在 `pre-revert-<id>/` 中。

## Session operations

Host 可以列出 Session、检查其事件、继续执行它们，或删除它们。这些都是围绕同一套磁盘布局提供的便捷操作。

轮次执行见 [Agent Loop](agent-loop.md)，不同 Host 如何暴露 Session 操作见 [Host Apps](../architecture/host-apps.md)。
