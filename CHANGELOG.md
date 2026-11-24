# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- Agent loop: recover once from model output-limit responses with an auditable `model.recovery` event and frozen prompt notice
- Release pipeline: CI builds for Linux x86_64, macOS aarch64, Windows x86_64
- Install scripts for Linux/macOS (`install.sh`) and Windows (`install.ps1`)
- Config: `[update]` section with source, channel, and multi-source support
- Config migration for `[update]` via `config_patch_defaults`
- Version injection at build time (`kuku --version`)
- Makefile with build-web, build, build-all targets

## [0.1.0-alpha.4]

这版可以在终端粘贴图片、用 ChatGPT 订阅登录，并在对话中切换模型和工作区权限。Linux ARM64 首次提供安装包。

### 新增功能

- **TUI 图片输入。** 粘贴 PNG、JPEG 或本地图片路径。草稿里的 `[Image #1]` 可以整体移动和删除。发送前可选压缩或原图，三秒无操作时自动选压缩。
- **Agent 图像能力。** 主 Agent 和子 Agent 能识别自身及候选模型的输入、输出模态。声明支持图像的 DeepSeek Responses 路线可以接收图片和使用 `view_image`。SDK 和 Session HTTP API v1 支持图片附件。
- **ChatGPT 登录。** 支持多个账号、在远程主机发起浏览器授权，以及按账号获取可用模型。命令为 `kuku auth login chatgpt`。
- **会话内调整模型。** 用 `/model`、`/thinking` 和 `/speed` 调整当前 Agent。模型连接或服务档位变化时，发送前提示确认并保留草稿。自定义连接和模型路线可配置请求字段。
- **工作区权限和异步审批。** 用 `F3` 或 `/permissions` 在全开放、询问和只读之间切换，并保存工作区选择。用 `F2` 查看待审批操作及参数，等待时仍可操作终端。批准后，前台或后台 `run_js` 会继续执行。
- **终端提示音。** 提问、审批、中断和轮次完成可发出声音，在 `/notifications` 中可分别设置。

### 体验改进

- 初次启动直接进入主 TUI。未配置模型时可打开 `/model`，完成设置后在原窗口重连。
- Agent 创建会报告实际工作区与工具。委派结果可按投递回执查找，连续给同一 Agent 发任务时也能区分结果。
- 会话历史分批读取，自动压缩以可用输入额度为准。终端可显示思考用时，并展开脚本执行记录、命令参数和带行号的工具输出。
- `search_output` 可以在保留的工具输出中按字节范围搜索。原生工具和 `run_js` 共用工作区资源限制，SDK 宿主可为命令子进程配置内存上限。
- 内置和自定义 Prompt 统一使用 MiniJinja。模板可以按模态、工具集和权限模式渲染，项目指令和用户消息作为文字插入。

### Bug 修复

- 修复请求预算紧张时漏传已接纳图片，以及召回同文异图、切换 Agent、编辑排队输入和重开 Session 时的附件恢复问题。
- 修复流式思考片段的重放顺序和 OpenRouter Gemini 路线的输出引用参数。响应解析失败时会保留已收到的内容。新输入打断 `run_js` 时，可区分已完成、已取消和未派发的操作。
- 修复并发读取及恢复 Session 时的日志争用、会话查询返回无效 JSON，以及保留输出在会话重开后难以查找的问题。损坏的单条会话可从列表中隔离。

### 安装与升级

- 安装包提供终端客户端和 HTTP 服务，覆盖 Linux x86-64 与 ARM64、Apple Silicon macOS、Windows x86-64。WebUI 可从源码启用 `embedded-web-assets` 构建。[安装指南](https://github.com/Cloud370/kuku/blob/v0.1.0-alpha.4/docs/zh/start/install.md) · [WebUI 构建](https://github.com/Cloud370/kuku/blob/v0.1.0-alpha.4/docs/zh/reference/cli.md#kuku-server-与-kuku-web)
- 若要跟踪后续预发布，运行 `kuku config set update.channel alpha`。从源码构建使用 Rust Edition 2024。

## [0.1.0-alpha.3] - 2026-11-22

Developer preview for the native terminal and Session workflow.

### Highlights

- **Terminal and Sessions.** The TUI can resume a durable Session, switch Agents, manage queued inputs, and inspect background Jobs and their output.
- **History and context.** Reversible context compaction keeps earlier history available. Use Ctrl+G to switch between saved Turns, bring the original message back as a draft, or preview optional file restoration. File restoration uses verified built-in edits and shows the affected files and line counts.
- **Tools and output.** Launch programs with literal arguments, compose deterministic operations in `run_js`, and search retained output by byte offset. Recognized command output gets a bounded diagnostic summary with links to original bytes.
- **Storage and inspection.** Large Sessions pack cold history in seekable gzip frames inside one journal. The CLI can page, query and export the committed events as JSONL.

### Measured on a local fixture

A 509 MiB synthetic Session packed to 65 MiB. In the paired cold-switch test with a different request byte limit, the server took 88–92 ms to commit the switch, down from 2.19–2.31 s. The TUI displayed the restored draft in 0.22 s in the local run.
