# Glossary

## 核心运行时术语

| Term | Meaning |
|---|---|
| `Workspace` | 一次运行使用的项目根目录 |
| `kuku home` | 运行时 home 目录，默认是 `~/.kuku` |
| `Session` | 以目录形式存储的一段已持久化执行历史 |
| `Agent` | 同一 Session 内拥有稳定地址、独立上下文和待处理输入的执行成员 |
| `Turn` | Agent 接纳的一轮工作，可包含多次模型请求 |
| `Step` | 一次逻辑模型请求及其响应触发的工具调用 |
| `events.jsonl` | 追加式 Session 事实日志 |
| `Memory` | 存储在 `memory.md` 文件中的长期上下文 |

## 模型与上下文术语

| Term | Meaning |
|---|---|
| `provider` | 模型 API 适配器 |
| `tier` | 命名的模型预设，例如 `strong`、`balanced` 或 `light` |
| `think level` | provider 推理级别：`off`、`low`、`medium` 或 `high` |
| `handoff` | 在长历史被压缩时写入的摘要 |

## 扩展术语

| Term | Meaning |
|---|---|
| Agent 预设 | 可发现的定义，为新建 Agent 提供初始指令、模型档和工具 |
| `Skill` | 加载到当前 Session 中的一组打包指令 |
| `package` | hooks、skills 和相关资源的打包集合 |
| `hook` | 由运行时生命周期事件触发的外部进程 |

## Tooling 术语

| Term | Meaning |
|---|---|
| `tool` | 带 schema 和风险等级的可调用能力 |
| `risk` | 工具风险类别，例如 `read`、`edit`、`command` 和 `session` |
| `permission` | Tool 执行前的运行时授权决定 |
| `wire event` | server 发出的面向客户端的 NDJSON 事件 |
