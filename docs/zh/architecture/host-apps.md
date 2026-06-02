# Host Apps

host app 把 SDK 事实呈现给用户。它们不会替代 SDK 的运行时模型。

## Repo layout

```text
crates/
|- kuku/
|- kuku-cli/
`- kuku-server/

apps/
|- kuku/
|- web/
`- tauri/  planned
```

## Responsibilities

### SDK

SDK 负责：

- Session 状态和事件持久化
- 上下文重建
- provider adapter
- Tool 分发
- 权限决策
- wire 事件转换

### Hosts

host 负责：

- 命令解析
- UI 布局
- 传输细节
- 权限提示和其他交互界面
- Session 历史和流式输出的展示

## Current hosts

### CLI

`kuku-cli` 为运行、Session 检查、配置操作、Prompt 资源检查，以及 Agent 和 Skill 的目录视图实现命令行为。

### Server

`kuku-server` 运行一个长期存活的 HTTP 进程，并把 run 事件作为 NDJSON 流输出。它会把活动中的 run 保存在内存里，但持久状态仍然来自 SDK 写入的 Session 文件。

### Web

`apps/web` 是一个前端 SPA，通过 HTTP 与 `kuku-server` 通信。

### Tauri

`apps/tauri` 计划作为桌面壳层，通过嵌入 `kuku-server` 来工作，而不是直接调用 SDK。

## Design rule

没有 host 会嵌入另一个 host。每个 host 都直接调用 SDK，或者像 Tauri 一样把 server library 作为自己的传输层嵌入。

主分层见 [Architecture Overview](overview.md)，package 和 hook 如何围绕运行时挂接见 [Extension Runtime](extension-runtime.md)。
