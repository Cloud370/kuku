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

## [0.1.0-alpha.8]

这版修复了打断后会话卡住、已排队消息不知道该如何继续的问题，也改善了 OpenRouter 的思考显示和大文件阅读。

### 体验改进

- **待发消息的状态和操作放在一起。** 输入框上方显示保存在后台的消息及处理时机（下一步或下一轮）。暂停且可恢复时直接显示「继续会话／Agent」；工具结果未确认时只提供查看，不误导用户继续。重连或重开终端仍按后台的真实队列显示，不重复发送。
- **少为文件和工具输出翻页。** 模型直接调用 `read_file` 的默认单页从 8 KiB 提高到 32 KiB，仍按完整行续读。主动读取保留输出时，`read_output` 默认 8 KiB，可显式请求最多 32 KiB 的原始数据；返回 JSON 另有 64 KiB 上限。搜索、命令首屏和脚本内部读取的预算不变。

### Bug 修复

- **打断等待子 Agent 不再卡住主会话。** 取消 `list_agents` 的只读等待时会记录取消结果，不停止子 Agent，也不会重跑旧工具。运行命令、写文件等操作若没有确认结果，仍会阻止正常恢复。服务确认中断之后，即使清理失败，也不会把中断命令显示成回执未确认。
- **OpenRouter 能显示更多模型提供的可读思考。** Chat Completions 可显示 Gemini 4 Neon 等模型返回的可读思考，避免流式内容重复；Anthropic Messages 会为支持的 Claude 路线请求并显示思考摘要。仅有签名或加密块的内容不显示，也不会回填旧会话。

### 安装与升级

- 安装包提供 Linux x86-64／ARM64、Apple Silicon macOS 和 Windows x86-64 版本。安装后用 `kuku --version` 核对版本；使用独立或远程服务时，建议客户端与服务端同步升级。参阅[升级指南](https://github.com/Cloud370/kuku/blob/v0.1.0-alpha.8/docs/zh/guides/upgrade-kuku.md)。

## [0.1.0-alpha.7]

这版扩展了普通会话的图片输入，同时改进终端里的打断、消息排队、Agent 切换和工具结果阅读。alpha.6 候选未通过 Windows 完整验收、没有发布；本版接续 alpha.5，并修正完整验收的总时限及失败诊断。

### 新增功能

- **更多模型路线支持图片。** 声明支持图像输入的 Chat Completions、Anthropic Messages 和 DeepSeek Responses 路线可接收用户图片，也可在允许工具调用时使用 `view_image`。其他 Responses 路线通过图像测试或显式声明图像输入能力后，可接收用户附件，但不支持工具结果图片。每条消息最多四张 PNG/JPEG，提交图片每张不超过 1 MiB；工具图片仍受文件读取授权和请求字节预算限制。
- **模型设置保留图像测试结果。** 保存成功验证的精确模型和路线后，下次可直接粘贴图片。手工声明优先，失败测试不判定模型不支持图像。CLI 的 `config test` 仍只报告结果、不修改配置。

### 体验改进

- **打断和停止分开。** 输入框聚焦时按 Esc 打断当前 Turn，成功清理后按原顺序继续已排队输入，不重试被打断的 Turn，也不发送草稿；明确 Stop 才暂停队列。排队中的 Agent 消息显示发送者，可以确认撤回，但不能编辑或变成草稿。
- `/model` 可直接输入或粘贴筛选，`Alt+D` 设为新 Session 的默认模型，`Alt+E` 编辑。Agent 列表与快捷切换按创建顺序排列，主 Agent 单独标记。
- 活动状态更准确地显示当前请求、工具、等待与失败；最新可读工具活动自动展开，长活动保留有界预览，命令和后台 Job 可查看实际参数、耗时及原始输出。历史阅读位置更稳定，卡片时间按终端所在时区显示，同轮统计不再重复占用空间。
- 改进 Agent 工具指引：单步操作优先直接调用已有工具，`run_js` 用于组合操作或处理结果；普通委派沿用预设或默认可用工具，不改变授权规则。

### Bug 修复

- 修复普通发送被误显示为需要恢复的排队消息、重连后执行和 Agent 列表状态不准确，以及过期问题或审批操作可能被重复提交的问题。答案草稿按具体问题保留，问题和回答更易阅读。
- 修复历史回退被不可见 Agent 消息阻塞，以及图片编码升级后重开已有 Session 时可能无法继续使用已有图像的问题。

### 安装与升级

- **Session HTTP 协议升级到 3。** 终端、Web 客户端和服务端须对应升级；独立或远程服务请同步更新。已有受管服务在空闲时自动切换。SDK 使用者需留意 `MediaAdapterSupport`、`CapabilitySource` 的新增枚举值及图像探测字段。
- **Provider Trace 默认改为完整记录。** 保存有界的推理请求、响应头、SSE 及有限的 HTTP 错误正文。可识别的凭据会脱敏，但提示词、图片及工具结果仍可能留在本地 trace；如不希望保存，启动服务前设置 `KUKU_PROVIDER_TRACE=0`。包含图片的请求出错时，Session 不再复制可能回显图片的上游错误正文。
- 预发布覆盖 Linux x86-64 / ARM64、Apple Silicon macOS 和 Windows x86-64；安装后用 `kuku --version` 核对版本。参阅[升级指南](https://github.com/Cloud370/kuku/blob/v0.1.0-alpha.7/docs/zh/guides/upgrade-kuku.md)。跟踪后续预发布可运行 `kuku config set --patch 'update.channel = "alpha"'`。

## [0.1.0-alpha.5]

这版把模型配置、内置帮助和中断后的继续操作接到同一套终端流程。首次打开 kuku 时，如果尚未配置模型，会直接进入配置页面。

### 新增功能

- **查看和验证模型路线。** `kuku config models` 查看已配置的连接和精确模型 ID，也可查询指定连接的模型列表。`kuku config validate` 本地校验配置并显示服务加载状态；`kuku config test` 可实际探测文本、图像输入或工具调用。探针可针对草稿配置运行，不会保存配置或启动服务，模型请求可能产生费用。
- **Agent 按需查看当前模型目录。** `list_models` 从运行配置列出连接、精确模型 ID 和能力，不用用户猜模型名，也不会发请求探测上游。
- **用脚本运行和继续任务。** `kuku run --prompt-file PATH` 从文件读取提示词，使用 `-` 可从标准输入读取。用 `--session` 指定会话，或用 `--continue` 继续最近的会话。可分别读取最终结果或逐行事件。
- **随服务提供版本匹配的帮助。** 从 `/` 菜单选择 `kuku-docs`，或直接提问怎么恢复 Session、配置模型。Agent 从真实目录用普通文件工具读取资料，沿用原有授权规则；恢复既有 Turn 时仍使用当时的版本。

### 体验改进

- 默认 Session 的首轮目录保留所有 Skill 名称；描述放得下就完整保留，缩短时会明确标记。新增、更新和移除在下一轮通知，不改写已冻结的前缀。
- 对应版本的 Skill 正文已在上下文中时不重复加载。用户显式选中的 Skill 在当前 Turn 压缩后仍可用；轮次进行中安装新 Skill 不会打断当前工作。
- 缺少模型配置与服务连接失败会分别提示，避免把首次设置误当成服务离线。

### Bug 修复

- 修复中断后回退再发送时，消息一直卡在队列的问题。排队期间被删除的 Skill 不会触发模型请求，重开 Session 后仍可回退对应输入。
- 修复 macOS 终端调整窗口大小后，紧接着的快捷键可能被延迟到下一次输入才交付的问题。

### 安装与升级

- **旧模型配置需要更新。** 默认模型用 `default_model = { connection = "…", model = "…" }` 指定，模型设置改用 `[model.<连接名>."<模型 ID>"]`。脚本显式选模型时同时传 `--connection` 和 `--model`。`kuku init` 已移除，直接运行 `kuku`，或在 TUI 用 `/model`。
- **脚本输出有变化。** `kuku run --json` 现在只写一个最终结果；需要 alpha.4 的逐行 JSON 事件时，改用 `kuku run --stream-json`。
- 预发布覆盖 Linux x86-64 / ARM64、Apple Silicon macOS 和 Windows x86-64。安装后运行 `kuku --version` 核对版本。升级已有安装时可参阅[升级指南](https://github.com/Cloud370/kuku/blob/v0.1.0-alpha.5/docs/zh/guides/upgrade-kuku.md)。
- 若要跟踪后续预发布，可设置 `kuku config set update.channel alpha`。

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
