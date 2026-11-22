# kuku 文档

kuku 是处于测试阶段的文件原生 Rust Agent SDK。在工作区运行 `kuku` 即可从终端开始任务，也可以从浏览器回到同一个 Session。[安装](start/install.md) · [第一个任务](start/quickstart.md) · [运行服务](guides/run-the-server.md)

**首次正式发布前，kuku 只支持当前版本的公开 API 和 Session 格式。** 更新可能影响既有的 SDK、CLI、HTTP 或流式协议集成，也可能导致旧 Session 无法打开。

Linux 空闲服务端实测约 **16 MiB RSS**。SDK 从日志内嵌的检查点重开已封存的 Session。[性能实测](how-it-works/performance.md)

## 栏目

- [入门](start/index.md) - 安装 kuku、完成第一次运行、设置配置。
- [使用指南](guides/index.md) - 按任务流程完成常见操作。
- [工作原理](how-it-works/index.md) - 理解运行模型、Session 流程、Memory 和权限。
- [参考手册](reference/index.md) - 查命令、配置、环境变量、格式、API 和术语。
- [架构设计](architecture/index.md) - 查看 crate 边界、host apps、Prompt 装配和内部设计。
- [参与开发](contributing/index.md) - 遵循开发、测试和发布流程。

## 推荐阅读路径

1. [安装 kuku](start/install.md)
2. [运行第一个任务](start/quickstart.md)
3. [配置模型连接](start/configuration.md)
4. [运行与继续任务](guides/run-a-task.md)
5. [CLI 命令参考](reference/cli.md)

## 关键页面

- [核心方向 — 先读这一页](architecture/core-direction.md) — 核心开发的精简目标基线，不代表已交付行为。
- [为什么使用 kuku](how-it-works/why-kuku.md)
- [Agent 如何执行任务](how-it-works/agent-loop.md)
- [文件布局](reference/file-layout.md)
- [HTTP 服务接口](reference/server-api.md)
- [开发流程](contributing/development.md)
