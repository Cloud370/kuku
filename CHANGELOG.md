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

这一版带来 ChatGPT 登录、可切换的工作区权限，以及首个 Linux ARM64 安装包。

### 新增功能

- **ChatGPT 登录。** 支持保存多个账号、从无桌面主机发起浏览器授权，以及按账号获取可用模型。CLI 可用 `kuku auth login chatgpt` 登录。
- **在会话中调整模型。** 用 `/model`、`/thinking` 和 `/speed` 设置当前 Agent。模型连接或服务档位与上次请求不同时，发送前会提示确认，草稿会保留。自定义连接和模型路线可以设置请求头和 JSON 请求字段。
- **工作区权限和异步审批。** 默认全开放，用 `F3` 或 `/permissions` 可切换为询问或只读，并保存工作区的选择。`F2` 可查看待审批操作及参数，等待时仍可使用 TUI。批准后，前台或后台的 `run_js` 可以从等待处继续。
- **终端声音。** 提问、审批、中断和轮次完成可发出提示音，可在 `/notifications` 分别设置。
- **工具输出搜索。** 新增 `search_output`，可直接在保留的工具输出中按字节范围搜索，继续读取后续匹配。
- **Linux ARM64。** 新增 ARM64 静态安装包，安装脚本会自动识别架构。

### 体验改进

- 初次启动直接进入主 TUI。模型未配置时可用 `/model` 打开设置，保存后在原窗口重连。
- Agent 创建会返回实际工作区和工具。委派后可以用投递回执查询对应回合与结果，后续再次给同一个 Agent 发任务也能区分两次结果。
- 长会话分批读取历史，自动压缩按可用输入额度计算。终端显示思考用时，可展开查看脚本执行记录、命令参数和带行号的工具输出。
- 大文件搜索采用有界扫描并按页返回。直接工具和 `run_js` 共用工作区资源限制，SDK 宿主可以为命令子进程配置内存上限。
- 新输入中断 `run_js` 时，工具结果会注明原因，并区分已完成、已取消和未派发的操作。

### Bug 修复

- 修复 OpenRouter Gemini 路由上 `read_output` 和 `save_value` 的参数格式，并修复流式思考片段重放顺序。响应解析失败时，已收到的部分内容会保留在历史中。
- 修复并发读取及恢复 Session 时的日志争用。损坏的单条 Session 记录可从列表隔离，其他 Session 仍可浏览。
- 修复会话查询可能返回无效 JSON、较深的工具输出内容难以直接搜索等问题。保存的短输出 ID 在重开会话后仍可使用。

### 升级须知

- 发布包提供 TUI 和 server API。需要 WebUI 时，可从源码启用 `embedded-web-assets` 构建。
- 源码工作区采用 Rust Edition 2024。SDK 和 HTTP 集成应核对旧回滚接口及子会话委派接口的移除，使用当前的会话历史和 Agent 消息接口。

## [0.1.0-alpha.3] - 2026-11-22

Developer preview for the native terminal and Session workflow.

### Highlights

- **Terminal and Sessions.** The TUI can resume a durable Session, switch Agents, manage queued inputs, and inspect background Jobs and their output.
- **History and context.** Reversible context compaction keeps earlier history available. Use Ctrl+G to switch between saved Turns, bring the original message back as a draft, or preview optional file restoration. File restoration uses verified built-in edits and shows the affected files and line counts.
- **Tools and output.** Launch programs with literal arguments, compose deterministic operations in `run_js`, and search retained output by byte offset. Recognized command output gets a bounded diagnostic summary with links to original bytes.
- **Storage and inspection.** Large Sessions pack cold history in seekable gzip frames inside one journal. The CLI can page, query and export the committed events as JSONL.

### Measured on a local fixture

A 509 MiB synthetic Session packed to 65 MiB. In the paired cold-switch test with a different request byte limit, the server took 88–92 ms to commit the switch, down from 2.19–2.31 s. The TUI displayed the restored draft in 0.22 s in the local run.
