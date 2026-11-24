# kuku 文档

kuku 是高性能、低内存占用的文件原生 Agent 运行时与 Rust SDK。即便在一台只有 371 MiB 内存的 ARM64 设备上，它也能在终端里打开并继续一个上千轮的会话，同时顺畅翻看旧历史。[性能实测](how-it-works/performance.md#低内存-arm64-设备)

**首次正式发布前，kuku 只支持当前版本的公开 API 和 Session 格式。** 更新可能影响既有的 SDK、CLI、HTTP 或流式协议集成，也可能导致旧 Session 无法打开。

## 栏目

- [入门](start/index.md) — 安装 kuku、完成第一次运行并配置模型。
- [使用指南](guides/index.md) — 任务运行、Session、Memory、Agent、Skill 与服务的分步操作。
- [工作原理](how-it-works/index.md) — 文件原生模型、Agent 执行循环、Session 生命周期、权限机制与性能实测。
- [参考手册](reference/index.md) — CLI 命令、配置、环境变量、工具、文件格式、SDK 与服务 API。
- [架构设计](architecture/index.md) — crate 边界、宿主应用、Prompt 装配与内部设计。
- [参与开发](contributing/index.md) — 开发流程、测试规范、代码风格与发布流程。

## 完成第一个任务

- [从终端开始](start/quickstart.md)，配置一个模型并发送第一条消息。
- [从脚本运行](guides/run-a-task.md)，配置并提交无头任务，再读取结果。
- [嵌入 Rust SDK](reference/sdk.md#发送首个请求)，打开 Session 并发出首个请求。

## 关键页面

- [核心方向 — 先读这一页](architecture/core-direction.md) — 核心开发的精简目标基线，不代表已交付行为。
- [性能实测](how-it-works/performance.md)
- [会话文件与 Rust SDK](how-it-works/why-kuku.md)
- [Agent 如何执行任务](how-it-works/agent-loop.md)
- [文件布局](reference/file-layout.md)
- [HTTP 服务接口](reference/server-api.md)
- [开发流程](contributing/development.md)
