# Module Contracts

本页定义 SDK 各模块分别拥有什么、可以依赖什么，以及不能做什么。

## Core Rule

每个模块都应该只有一个清晰职责。依赖规则的目标，是让账本模型保持可理解且不易被误用。

## Ownership Map

| Module | Owns | May depend on | Must not |
|---|---|---|---|
| `query/` | 单条执行链、turn 执行、slot 生命周期 | 所有其他模块 | — |
| `conversation/` | conversation address、binding、reducer、conversation 作用域状态 | `event/`, `context/` | provider 逻辑、host 传输 |
| `agent/` | agent 定义、catalog、registry、委派运行准备 | `conversation/`, `tool/`, `query/`, `session/` | 不相关的 host 关注点 |
| `context/` | message replay、provenance、rollback 规划 | `event/`, `conversation/`, `prompt/`, `config/` | provider 调用、权限决策 |
| `provider/` | 模型 API 协议转换 | canonical messages、tool schemas、config | session 状态、event store、permissions |
| `tool/` | 定义、registry、dispatch、built-ins | `event/`, `conversation/`, 共享 context 类型 | provider 协议、slot 调度策略 |
| `permission/` | 运行时 gate 决策 | `event/`, `conversation/` | tool 执行、provider 逻辑、session 归属 |
| `session/` | 运行归属、Agent 投递／调度、路径、查询、清理 | `query/`, `event/`, `permission/`, `config/` | provider 逻辑、prompt 归属 |
| `event/` | 事件类型与 `events.jsonl` 存储 | — | provider、tools、permissions |
| `prompt/` | prompt 资源与渲染 | — | 运行时决策、session 状态 |
| `config/` | 配置解析、校验、默认值、patch | — | provider 逻辑、tool 执行 |
| `skill/` | skill 定义、loader、registry、loaded-skill 恢复 | `prompt/`, `event/` | provider 逻辑、host 传输 |
| `plugin/` | package 发现、manifest、hook 执行、registry | `event/`, `config/`, `skill/` | provider 逻辑、permission 逻辑 |
| `notice/` | inbox、中断、open conversations、drift 等 runtime notices | `event/`, `conversation/`, `prompt/`, `skill/`, `agent/` | provider 逻辑 |

## Important Boundaries

### `query/`

`session/runtime/` 拥有公开 Session 操作、唯一 writer、Agent inbox、每 Agent 唯一活动执行代次、观察和异步清理。它调度已有 `query/` 状态机作为唯一执行器，观察者不推进执行。`query/` 推进单条链，拥有准备好的模型 Step 与工具 slot。模型／工具 I/O 不持有 Session 提交锁。

### `conversation/`

`conversation/` 保留事件 codec 与历史归约表示。公开 Session Agent 身份由 `session/runtime/` 校验并拥有，使用扁平地址；旧斜杠 conversation 语法不创建隐藏 Agent 身份。

### `agent/`

`agent/` 负责联系人卡片发现和委派运行准备，但不拥有账本本身。

### `event/`

`event/` 是唯一允许写入 `events.jsonl` 的模块。这是 SDK 最重要的完整性边界之一。

### `context/` and `prompt/`

`prompt/` 负责渲染资源。`context/` 负责组装请求并回放 conversation 历史。把它们拆开，可以避免把文本模板和运行时状态决策混在一起。

Session 运行时通过有界事件页选择上下文。活动 cursor 只保留当前 Agent 的已准备段，并增量读取后续事实；空闲 Agent 状态不缓存对话历史。原始 Prompt 资源与已渲染的静态材料在模型派发前随段提交，因此重开不依赖今天的模板、指令或记忆文件。新选中的输入与精确工具结果视图只接纳一次，每个 Step 仅记录段／消息边界、源事实水位、有效限额和线路哈希。没有已准备 Step 边界的部分接纳记录不替代已提交前缀。Handoff 与不兼容路由变化通过后继段选择完整 Turn；跨连接、协议、模型或端点时移除供应商原生重放状态。

### `provider/`

Provider 适配器负责协议转换、端点能力校验、认证和传输关联，不应增长账本、conversation 或 permission 行为。按配置名称选择连接；线路协议不用于选择第一条匹配连接。请求设置、实际发送的设置与可选 provider 回报保持为不同事实。运行时提供稳定的 Session／Agent／连接关联和已准备的上下文；适配器在派发前冻结线路请求，重放时不渲染行为 Prompt 资源。供应商原生的推理重放数据附在 assistant 消息上，只供匹配协议使用。

原生重放保留 Messages 的 thinking 签名和块顺序、Responses 的推理／输出项以及 Chat 的 reasoning details。不兼容的协议需要后继上下文段。流解码保持跨网络块的 UTF-8，接受 SSE 行结束形式和多行 data，并在调用方丢弃流后关闭 HTTP 读取任务。单个 SSE 事件限制为 2 MiB；保留的原生重放和原始 usage 分别限制为 512 KiB。超限在依赖工具派发前明确失败。各协议的 `ProviderUsage.input_tokens` 均表示包含可获得缓存读／写计数的总输入；缓存计数是其子集，原始 usage 对象及可选的模型、上游、服务、思考和费用回报分别保留。未获 provider 确认的内容保持未知。

## Related Pages

- [Prompt Assembly](prompt-assembly.md)
- [Extension Runtime](extension-runtime.md)
- [Code Style](../contributing/code-style.md)
