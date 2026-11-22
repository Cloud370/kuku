# Package 格式

Package 用于打包 Hook、Skill 及配套资源。

## 目录位置与结构

- 用户级：`~/.kuku/packages/<name>/`
- 项目级：`<workspace>/.kuku/packages/<name>/`（同名时覆盖用户级）

```text
.kuku/packages/<name>/
├── kuku.toml
├── hooks/
├── skills/
├── .mcp.json
└── bin/
```

`kuku.toml` 是 Package 清单文件，也是唯一权威定义。

## `[package]`

```toml
[package]
name = "security-guard"
version = "1.2.0"
description = "Safety net for production sessions"
homepage = "https://github.com/user/kuku-security"
repository = "https://github.com/user/kuku-security"
```

- `name`（必填）：1–64 个字符，仅限小写字母、数字和连字符。
- `version`（必填）：符合 SemVer 规范。

## `[[hooks]]`

```toml
[[hooks]]
event = "tool.pre_execute"
command = "hooks/pre-check.sh"
matcher = 'tool_name == "run_command"'
timeout_seconds = 30
chain = false
env = ["MY_TOKEN"]
```

或绑定多个事件（使用 `events` 时不要再写 `event`）：

```toml
[[hooks]]
events = ["tool.pre_execute", "tool.post_execute"]
command = "hooks/audit-tool.sh"
```

| 字段 | 必填 | 含义 |
|---|---|---|
| `event` 或 `events` | 是 | 触发事件名或列表 |
| `command` | 是 | 相对 Package 根目录的可执行路径 |
| `matcher` | 否 | 布尔过滤表达式 |
| `timeout_seconds` | 否 | 默认 `30`，硬上限 `600` |
| `chain` | 否 | 是否接收前一个 Hook 的输出 |
| `env` | 否 | 额外透传的环境变量名列表 |

**已实现事件**：`session.start`、`session.end`、`tool.pre_execute`、`tool.post_execute`、`model.pre_request`、`model.post_response`。

**Matcher 语法**：支持运算符 `==`、`!=`、`contains`、`&&`、`||`；常用变量包括 `event`、`tool_name`、`tool_call_id`、`args.<field>`、`status`、`summary`、`tier`、`text`、`stop_reason`。

## Hook 协议

- **stdin**：包含最小上下文（如 `event` 与 `session_dir`）的 JSON 对象。
- **stdout**：合法 JSON 按结构化输出解析；纯文本自动包装为 `{"additional_context": "..."}`。
- **退出码**：`0` 表示成功，`2` 表示阻止当前操作，其他非零码作为非阻断错误记录。

结构化 stdout 字段：

| 字段 | 含义 |
|---|---|
| `block` | 阻止当前操作 |
| `updated_args` | 替换工具参数 |
| `updated_result` | 替换工具结果 |
| `additional_context` | 向下一轮注入额外上下文 |
| `permission_override` | 计划中的权限覆盖字段 |

## MCP

`.mcp.json` 采用标准 MCP 格式；运行时 MCP 集成仍在计划中，尚未定型。
