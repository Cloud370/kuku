# kuku

[English](../../README.md)

> 极简终端编程助手，以文件为核心

kuku 是一个以文件为核心的终端编程助手。没有数据库，没有服务器，没有隐藏状态。一切——配置、记忆、会话、技能——都是你可以直接阅读、编辑和版本控制的文件。

## 为什么选择 kuku

- **零基础设施** — 无需数据库，无需服务器。所有状态存储在 `~/.kuku/` 下的人类可读文件中。
- **可检查** — 配置、技能、提示词和记忆都是纯文件，没有任何隐藏。
- **无隐藏状态** — 运行时状态全部在磁盘上，没有看不见的内存缓存。
- **追求极致缓存命中** — 最小系统提示词（约 3K token），为最大缓存命中率而设计。

## 工程质量对比

| | kuku | Claude Code | Codex | OpenCode |
|--|------|-------------|-------|----------|
| 二进制大小 | **~10 MB** | ~250 MB | ~80 MB | ~50 MB |
| 依赖数 | **~15** | ~80 | ~280 | ~100 |
| 配置 | 1 个 TOML 文件 | JSON + flags + feature gates | 96 字段，9 层 TOML | 57+ 字段，9 层 |
| 系统提示词 | **~3K tokens** | ~30K tokens | ~9K tokens | ~15K tokens |
| 记忆存储 | 1 个 markdown 文件 | Markdown + YAML | SQLite + JSONL + Markdown | SQLite + JSON（无独立记忆功能） |

> 基于 2026 年 5 月的源码分析。系统提示词包含会话初始化时注入的全部 token——系统指令、工具定义、运行时上下文。

## 功能

| 功能 | 状态 |
|------|------|
| 文件原生 agent 循环 | 已完成 |
| 工具（读取、搜索、编辑、写入、执行） | 已完成 |
| 技能系统 | 已完成 |
| 记忆（持久化，人类可读） | 已完成 |
| 子代理（隔离会话） | 已完成 |
| 权限系统（多层级） | 已完成 |
| 多模型供应商（Anthropic、OpenAI） | 已完成 |
| 流式输出 | 已完成 |
| 命令行界面 | 已完成 |
| HTTP 服务器 | 已完成 |
| MCP 支持 | 计划中 |
| 扩展系统 | 计划中 |
| Web 界面 | 计划中 |
| 桌面端 | 计划中 |

## 快速开始

```bash
cargo install --git https://github.com/Cloud370/kuku
kuku run say hello
```

## 文档

[docs/en/](../en/)

## 许可证

以下许可证任选其一：

* MIT 许可证（[LICENSE-MIT](../../LICENSE-MIT) 或 http://opensource.org/licenses/MIT）
* Apache 许可证 2.0 版（[LICENSE-APACHE](../../LICENSE-APACHE) 或 http://www.apache.org/licenses/LICENSE-2.0）

由您选择。

### 贡献

除非您明确声明，否则您有意提交给本项目的任何贡献（如 Apache-2.0 许可证中所定义）均应按上述双许可证授权，不附加任何额外条款。
