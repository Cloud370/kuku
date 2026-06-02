# Module Contracts

本页定义每个 SDK 模块拥有什么、可以依赖什么，以及不能做什么。

## Core rule

每个模块都应该只有一个清晰职责。依赖规则的存在，是为了让这种 file-native 运行时更容易理解，也更不容易被误用。

## Ownership map

| Module | Owns | May depend on | Must not |
|--------|------|---------------|----------|
| `query/` | agent loop, `Run`, orchestration | all other modules | — |
| `context/` | message rebuild, provenance, rollback planning | `event/`, `prompt/`, `config/`, shared tool helpers | provider calls, permission decisions |
| `provider/` | protocol conversion for model APIs | canonical messages, tool schemas, config | session state, event store, permissions |
| `tool/` | definitions, registry, dispatch, built-ins | `event/`, shared context types | provider protocol, slot scheduling policy |
| `permission/` | runtime gate decisions | `event/` | tool execution, provider logic, session ownership |
| `session/` | paths, lock, list, delete, status | `event/` scan helpers | provider logic, model state |
| `event/` | event types and `events.jsonl` storage | — | provider, tools, permissions |
| `prompt/` | prompt assets and rendering | — | runtime decisions, session state |
| `config/` | config parsing, validation, defaults, patching | — | provider logic, tool execution |
| `skill/` | skill definitions, loader, registry | `prompt/` | runtime decisions, session state |
| `plugin/` | package discovery, manifests, hook execution, registry | `event/`, `config/`, `skill/` | provider logic, permission logic |
| `subagent/` | agent definitions, registry, child-session spawn | `tool/`, `query/`, `session/` | unrelated host concerns |
| `notice/` | drift detection and notice rendering | `event/`, `prompt/` | provider logic |

## Important boundaries

### `query/`

`query/` 是唯一可以依赖其他所有模块的编排层。它推进 run 状态机，并把各模块输出变成实时的 agent loop。

### `event/`

`event/` 是唯一会写入 `events.jsonl` 的模块。这是 SDK 最主要的完整性边界之一。

### `context/` and `prompt/`

`prompt/` 负责渲染资源，`context/` 负责装配请求。把它们分开可以避免把文本模板和运行时状态决策混在一起。

### `provider/`

provider adapter 只负责协议翻译。它们不应该膨胀出 Session 或权限行为。

## Related pages

- [Prompt Assembly](prompt-assembly.md)
- [Extension Runtime](extension-runtime.md)
- [Code Style](../contributing/code-style.md)
