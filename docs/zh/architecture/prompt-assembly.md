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

### System prompt (`catalog.system.text`)

稳定的运行时契约。它承载身份、硬规则和工作风格，但不承载工作区特定状态。

### Prelude messages（快照层 2–6）

Prelude 是可复用上下文的冻结快照。每个 turn 捕获一次，后续 turn 从
`PromptSnapshot` 事件恢复。各层：

| 层 | 内容 | 模板 |
|---|---|---|
| 2 | project policy | `blocks/project-policy.md` + 渲染后的项目指令和 model tiers |
| 3 | agent identity | `input.agent_instructions` |
| 4 | agent catalog + loaded skills | 由调用方通过 prelude push 注入 |
| 5 | tool guidance | `blocks/tool-guidance.md` |
| 6 | memory | `blocks/memory.md` + `memory/global.md` + `memory/project.md`（由 `enable_memory` 控制） |

### History

conversation 历史从 `events.jsonl` 中按当前 conversation address 重建，
过程中会先过滤 rolled-back 事件，再应用当前 handoff 边界。

### Per-turn 内容

当前 turn 的动态数据会注入到人类输入之前的最后一条 user message 中。
这些内容**不在**冻结快照中：

- runtime context（agent catalog、notices、skill catalog）包裹在 `runtime/context.md` 中
- response contract（surface、locale、preferences）用于 main conversation

出现在 runtime context 中的 notice 类型：agent directory、open conversations、
inbox、loaded skills、context drift。

## Assembly Order

```text
system prompt
prelude[0]       project_policy
prelude[1]       agent_identity
prelude[2]       agent catalog + skills（由调用方注入）
prelude[3..]     tool_guidance、memory*
messages[N..]    某个 conversation 的 replayed history
last user turn   runtime_context + human input
```

## Cache Behavior

稳定内容保留在 prelude 中，这样 provider 侧 prompt cache 可以跨 turns 和
conversations 复用。动态 runtime 数据放在最后的 user message 中，
这样变动时不会使整个前缀缓存失效。

## Asset Ownership

Prompt 文本位于 `crates/kuku/prompts/`，按类别组织：

| 目录 | 内容 |
|---|---|
| `blocks/` | 可复用模板块（project-policy、tool-guidance、memory、notices） |
| `agents/` | 带 YAML frontmatter 的 agent 定义 |
| `memory/` | 全局和项目 memory 模板 |
| `runtime/` | runtime context、handoff context 和 instruction 包装器 |
| `tools/` | 工具专用 system prompt |

`prompt/` 模块负责资源加载和渲染。`context/` 与 `conversation/` 模块
负责把某个 conversation 的历史、rollback 状态和 notices 组装成请求。

归属边界见 [Module Contracts](module-contracts.md)。面向读者的行为说明见
[Sessions](../how-it-works/sessions.md) 和
[Agents and Skills](../how-it-works/agents-and-skills.md)。
