# 会话文件与 Rust SDK

## 可直接核对的会话日志

每次运行都会在磁盘上留下完整的持久日志。用 `kuku show SESSION_ID` 看最新回复，用 `kuku events SESSION_ID` 查记录下来的输入、模型请求、工具调用和结果。Agent 改过哪些文件、跑过什么命令，随时能和工作区里的实际情况一一对照。[管理 Session](../guides/manage-sessions.md)

## 可重开、可并发的多 Agent Session

重开任意 Session 时，每个 Agent 的上下文、待处理请求和保留的工具输出都在。关掉 TUI、脚本客户端或浏览器后，服务会继续跑完已接下的工作。同一个 Session 里可以有多个 Agent 同时在同一个工作区里协作，各自拥有独立的地址、收件箱和上下文。[文件原生执行](file-native-model.md) · [运行服务](../guides/run-the-server.md)

## 可直接嵌入的 Rust 运行时

你可以直接把 Rust SDK 嵌进自己的程序里进程内跑 Agent、管日志，不需要额外起守护进程或 CLI 子进程。终端、脚本和浏览器通过 HTTP 服务使用的也是同一套运行时。[Rust SDK](../reference/sdk.md) · [服务端 API](../reference/server-api.md)
