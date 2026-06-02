# Prompt Assembly

本页说明 kuku 如何为 model 构建请求。

## Goal

Prompt 装配试图同时保持四件事成立：

- 稳定的运行时契约
- 静态上下文与动态上下文之间清晰的归属
- 良好的缓存行为
- 低格式噪音

## Layers

### System prompt

稳定的运行时契约。它承载身份、硬性规则和工作风格，不承载工作区特定状态。

### Prelude messages

最前面的 prelude message 承载可复用上下文：

| Position | Content |
|----------|---------|
| `messages[0]` | tool guidance |
| `messages[1]` | global `Memory` |
| `messages[2]` | project `Memory` |
| `messages[3]` | project context |

project context 包括项目指令、执行上下文和可用的 model tier。

### History

对话历史会从 `events.jsonl` 重建，过程中会过滤已回滚的 turn，并应用当前的 handoff 边界。

### Runtime context

当前 turn 的动态数据会放进最后一个用户消息中，位于人工输入之前。这包括目录信息以及上下文漂移之类的系统通知。

## Assembly order

```text
system prompt
messages[0]    tool_guidance
messages[1]    global_memory
messages[2]    project_memory
messages[3]    project_context
messages[4..]  rebuilt history
last user turn runtime_context + human input
```

## Cache behavior

稳定内容会保留在 prelude 中，这样 provider 侧的 Prompt 缓存就可以跨 turn 和 Session 复用它。动态运行时数据则放在最后一个用户消息里，这样它变化时不会让整个前缀失效。

## Asset ownership

Prompt 文本位于 `crates/kuku/prompts/`。`prompt/` 模块负责资源加载和渲染，而 `context/` 负责决定如何把这些渲染结果装配进请求。

关于这两个模块之间的边界，见 [Module Contracts](module-contracts.md)。如果想看面向读者的同一行为说明，见 [File-Native Model](../how-it-works/file-native-model.md) 和 [Memory](../how-it-works/memory.md)。
