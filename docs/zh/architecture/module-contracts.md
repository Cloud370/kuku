# Module Contracts

本页定义 SDK 各模块分别拥有什么、可以依赖什么，以及不能做什么。

## Core Rule

每个模块都应该只有一个清晰职责。依赖规则的目标，是让账本模型保持可理解且不易被误用。

## Ownership Map

| Module | Owns | May depend on | Must not |
|---|---|---|---|
| `query/` | 运行编排、turn 执行、slot 生命周期 | 所有其他模块 | — |
| `conversation/` | conversation address、binding、reducer、conversation 作用域状态 | `event/`, `context/` | provider 逻辑、host 传输 |
| `agent/` | agent 定义、catalog、registry、委派运行准备 | `conversation/`, `tool/`, `query/`, `session/` | 不相关的 host 关注点 |
| `context/` | message replay、provenance、rollback 规划 | `event/`, `conversation/`, `prompt/`, `config/` | provider 调用、权限决策 |
| `provider/` | 模型 API 协议转换 | canonical messages、tool schemas、config | session 状态、event store、permissions |
| `tool/` | 定义、registry、dispatch、built-ins | `event/`, `conversation/`, 共享 context 类型 | provider 协议、slot 调度策略 |
| `permission/` | 运行时 gate 决策 | `event/`, `conversation/` | tool 执行、provider 逻辑、session 归属 |
| `session/` | 路径、锁、列举、删除、session 级状态 | `event/`, `conversation/` 扫描辅助 | provider 逻辑、prompt 归属 |
| `event/` | 事件类型与 `events.jsonl` 存储 | — | provider、tools、permissions |
| `prompt/` | prompt 资源与渲染 | — | 运行时决策、session 状态 |
| `config/` | 配置解析、校验、默认值、patch | — | provider 逻辑、tool 执行 |
| `skill/` | skill 定义、loader、registry、loaded-skill 恢复 | `prompt/`, `event/` | provider 逻辑、host 传输 |
| `plugin/` | package 发现、manifest、hook 执行、registry | `event/`, `config/`, `skill/` | provider 逻辑、permission 逻辑 |
| `notice/` | inbox、中断、open conversations、drift 等 runtime notices | `event/`, `conversation/`, `prompt/`, `skill/`, `agent/` | provider 逻辑 |

## Important Boundaries

### `query/`

`query/` 是唯一允许依赖所有其他模块的编排层。它推进运行状态机，并把各模块输出转成实时 agent loop。

### `conversation/`

`conversation/` 拥有规范的低心理负担抽象：一本账本，多条线程。address 解析、连续性、binding 状态和 conversation reduction 都应归它，而不是归 `agent/` 或 `session/`。

### `agent/`

`agent/` 负责联系人卡片发现和委派运行准备，但不拥有账本本身。

### `event/`

`event/` 是唯一允许写入 `events.jsonl` 的模块。这是 SDK 最重要的完整性边界之一。

### `context/` and `prompt/`

`prompt/` 负责渲染资源。`context/` 负责组装请求并回放 conversation 历史。把它们拆开，可以避免把文本模板和运行时状态决策混在一起。

### `provider/`

provider adapter 只负责协议翻译，不应增长账本、conversation 或 permission 行为。

## Related Pages

- [Prompt Assembly](prompt-assembly.md)
- [Extension Runtime](extension-runtime.md)
- [Code Style](../contributing/code-style.md)
