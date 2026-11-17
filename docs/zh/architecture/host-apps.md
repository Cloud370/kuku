# 宿主应用

Session SDK 负责 Agent 执行、日志和输出保留。宿主打开 Session，提供工具授权策略，并用适合调用方的方式呈现结果。

| 入口 | 当前行为 |
|---|---|
| `kuku` 与 `kuku run` | CLI 在前台打开 SDK。裸 `kuku` 逐行读取输入，各次输入之间重开同一个 Session，输入 `exit`、`quit` 或收到 EOF 时退出。 |
| `kuku server` | 长期运行的 HTTP 进程持有打开的 Session。观察客户端断开后继续执行。API 提供持久事件分页、关联回复和显式取消。 |
| `kuku web` | 统一二进制启动 server，提供内嵌的浏览器应用。源码构建需要 `embedded-web-assets` 和构建好的 `apps/web/dist`。 |
| Rust 嵌入 | 应用可直接调用公开 `Session` SDK，自行配置、授权和观察运行。 |

CLI 实现在 `crates/kuku-cli`，server 在 `crates/kuku-server`，统一二进制在 `apps/kuku`，浏览器应用在 `apps/web`。[CLI 参考](../reference/cli.md) · [Server API](../reference/server-api.md) · [SDK 参考](../reference/sdk.md)

规划中的 Rust TUI、CLI 和 WebUI 将共用 server 持有的 Session。本地服务发现、窗口焦点与输入时机见[交互客户端计划](interactive-clients.md)。
