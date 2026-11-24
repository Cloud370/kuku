# 宿主应用

Session SDK 负责 Agent 执行、日志和输出保留。宿主打开 Session，提供工具授权策略，并用适合调用方的方式呈现结果。

| 入口 | 当前行为 |
|---|---|
| `kuku` 与 `kuku run` | 全屏终端界面和脚本命令共用 Rust HTTP 客户端。终端窗口持有草稿、焦点和滚动位置。 |
| `kuku server` | 长期运行的 HTTP 进程持有打开的 Session。观察客户端断开后继续执行。API 提供持久命令、快照、历史、观察和保留输出。 |
| Rust 嵌入 | 应用可直接调用公开 `Session` SDK，自行配置、授权和观察运行。 |

脚本命令位于 `crates/kuku-cli`，服务在 `crates/kuku-server`，HTTP 客户端在 `crates/kuku-client`，终端界面在 `crates/kuku-tui`。`apps/kuku` 组装统一二进制。[CLI 参考](../reference/cli.md) · [Server API](../reference/server-api.md) · [SDK 参考](../reference/sdk.md)

每个解析后的数据目录对应一个受管本地服务。客户端窗口传入目标工作区、Session 和 Agent，各自维护观察游标。浏览器客户端可用于开发验证。[交互客户端计划](interactive-clients.md) · [Web 客户端设计](web-client.md)
