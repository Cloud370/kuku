# Prompt Assembly

本页说明 kuku 如何构建一次模型请求。

## Goal

Prompt 组装同时追求四件事：

- 稳定的运行时契约
- 静态上下文与动态上下文的清晰归属
- 良好的缓存行为
- 较低的格式噪音

## Canonical Inputs

Prompt 组装是按 conversation 作用域进行的，而不是基于独立委派 Session 树。

- Session 账本是全局历史存储
- 活跃 conversation address 决定 replay 的切片
- 绑定的 agent 身份决定该 conversation 的工具和 notices

## Layers

### System prompt

稳定的运行时契约。它承载身份、硬规则和工作风格，但不承载工作区特定状态。

### Prelude messages

前几个 prelude message 携带可复用上下文：

| Position | Content |
|---|---|
| `messages[0]` | tool guidance |
| `messages[1]` | global `Memory` |
| `messages[2]` | project `Memory` |
| `messages[3]` | project context |

project context 包括项目指令、执行上下文和可用 model tiers。

### History

conversation 历史从 `events.jsonl` 中按当前 conversation address 重建，过程中会先过滤 rolled-back 事件，再应用当前 handoff 边界。

### Runtime context

当前 turn 的动态数据会放进人类输入之前的最后一条 user message。这里包括：

- `main` conversation 的 agent directory notice
- open conversation notice
- inbox notice
- loaded-skill notice
- pending-permission notice
- interrupted-turn notice
- context-drift notice

## Assembly Order

```text
system prompt
messages[0]    tool_guidance
messages[1]    global_memory
messages[2]    project_memory
messages[3]    project_context
messages[4..]  某个 conversation 的 replayed history
last user turn runtime_context + human input
```

## Cache Behavior

稳定内容保留在 prelude 中，这样 provider 侧 prompt cache 可以跨 turns 和 conversations 复用。动态 runtime 数据放在最后的 user message 中，这样变动时不会使整个前缀缓存失效。

## Asset Ownership

prompt 文本位于 `crates/kuku/prompts/`。`prompt/` 模块负责资源加载和渲染。`context/` 与 `conversation/` 模块负责把某个 conversation 的历史、rollback 状态和 notices 组装成请求。

归属边界见 [Module Contracts](module-contracts.md)。面向读者的行为说明见 [Sessions](../how-it-works/sessions.md) 和 [Agents and Skills](../how-it-works/agents-and-skills.md)。
