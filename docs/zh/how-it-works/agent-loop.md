# Agent Loop

只有写入 `events.jsonl` 的内容才算作 Session 事实。

## Turn flow

```text
turn.start
  -> user.input
  -> model.response
      stop_reason = tool_use ?
        yes -> tool.call -> permission.allow|permission.deny -> tool.result -> loop
        no  -> turn.end
```

## Per turn

1. kuku 追加写入 `turn.start` 和 `user.input`。
2. 它基于文件和已持久化事件重建模型上下文。
3. 它将模型响应流式传给 Host，并在完成后追加写入 `model.response`。
4. 如果响应结束这一轮，kuku 会追加写入 `turn.end`。
5. 如果响应请求 Tool，kuku 会追加写入 `tool.call`，记录权限决策，执行已允许的 Tool，追加写入 `tool.result`，然后为下一次模型调用重建上下文。

## Tool execution

彼此独立的 Tool 调用可以并行运行。kuku 在将结果写回事件日志时，会保留模型原始的 `tool.call` 顺序。

Subagent 在子 Session 中使用相同的循环。它们不会创建第二套运行时模型。

## Permissions inside the loop

模型可以请求某个 Tool，但运行时决定这个 Tool 是否可以执行。权限检查是运行时强制执行的机制，不是 Prompt 建议。

权限模型见 [Permissions](permissions.md)。

## Handoff and rollback

有两种 Session 行为会改变下一次模型调用可见的内容：

- 当上下文过大时，handoff 会把较早的历史压缩成结构化摘要
- rollback 会追加标记事件，使先前的轮次不再参与未来的上下文重建，也可以同时回退文件

这两种行为都见 [Sessions](sessions.md)。

运行时流和可观测性日志与 Session 事实日志分开。见 [Events](../reference/events.md) 和 [Sessions](sessions.md#observability-logs)。

## Maintainer view

本页描述的是可观察的运行时行为。关于 crate 边界和精确的组装顺序，请参见 [Architecture Overview](../architecture/overview.md)、[Prompt Assembly](../architecture/prompt-assembly.md) 和 [Module Contracts](../architecture/module-contracts.md)。
