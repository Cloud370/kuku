# 术语表

## 核心运行时

| 术语 | 含义 |
|---|---|
| `Workspace` | 一次执行对应的项目根目录 |
| `kuku home` | 运行时数据目录，默认 `~/.kuku` |
| `Session` | 以目录形式落盘的一份完整执行历史 |
| `Agent` | 同一 Session 内拥有固定地址、独立上下文和输入队列的成员 |
| `Turn` | Agent 接纳的一轮工作，可包含多次模型请求 |
| `Step` | 一次逻辑模型请求及其响应触发的工具调用 |
| `events.jsonl` | 追加式 Session 事实日志 |
| `Memory` | 存在 `memory.md` 中的跨 Session 长期记忆 |

## 模型与上下文

| 术语 | 含义 |
|---|---|
| `Provider` | 模型 API 适配器与连接配置 |
| `Tier` | 命名的模型档位，如 `strong`、`balanced`、`light` |
| `Think level` | 可移植的思考等级：`off`、`low`、`medium` 或 `high` |
| `Handoff` | 压缩长历史后开启后继上下文段的摘要边界 |

## 扩展机制

| 术语 | 含义 |
|---|---|
| Agent 预设 | 可自动发现的 Markdown 文件，定义初始指令、档位和工具 |
| `Skill` | 可按需加载到当前 Session 的打包指令集 |
| `Package` | 打包 Hook、Skill 与配套资源的扩展包 |
| `Hook` | 由运行时生命周期事件触发的外部进程 |

## 工具与通信协议

| 术语 | 含义 |
|---|---|
| `Tool` | 带参数 Schema 和风险等级的可调用能力 |
| `Risk` | 工具风险类别，如 `read`、`edit`、`command`、`session` |
| `Permission` | 工具执行前的运行时授权决定 |
| `Wire event` | HTTP 服务向客户端推送的 NDJSON 事件 |
