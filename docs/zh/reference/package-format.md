# Package Format

package 会打包 hooks、skills 和相关资源。

## 位置

- 用户级：`~/.kuku/packages/<name>/`
- 项目级：`<workspace>/.kuku/packages/<name>/`

同名时，项目级 package 会覆盖用户级 package。

## 布局

```text
.kuku/packages/<name>/
├── kuku.toml
├── hooks/
├── skills/
├── .mcp.json
└── bin/
```

`kuku.toml` 是 package manifest，也是规范的事实来源。

## `[package]`

```toml
[package]
name = "security-guard"
version = "1.2.0"
description = "Safety net for production sessions"
homepage = "https://github.com/user/kuku-security"
repository = "https://github.com/user/kuku-security"
```

规则：

- `name` 为必填，长度 1-64 个字符，只能包含小写字母、数字和连字符
- `version` 为必填，且必须符合 semver

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

或者：

```toml
[[hooks]]
events = ["tool.pre_execute", "tool.post_execute"]
command = "hooks/audit-tool.sh"
```

使用 `events` 时，`event` 必须不存在。

## Hook 字段

| Field | Required | Meaning |
|---|---|---|
| `event` or `events` | yes | 触发事件名或事件名列表 |
| `command` | yes | 相对于 package 根目录的可执行路径 |
| `matcher` | no | 布尔过滤表达式 |
| `timeout_seconds` | no | 默认 30，硬上限 600 |
| `chain` | no | hook 是否接收前一个 hook 的输出 |
| `env` | no | 需要额外透传的环境变量名 |

## 已实现的 Hook 事件

- `session.start`
- `session.end`
- `tool.pre_execute`
- `tool.post_execute`
- `model.pre_request`
- `model.post_response`

## Matcher 语法

运算符：

- `==`
- `!=`
- `contains`
- `&&`
- `||`

常用变量：

- `event`
- `tool_name`
- `tool_call_id`
- `args.<field>`
- `status`
- `summary`
- `tier`
- `text`
- `stop_reason`

## Hook 协议

stdin 是一个带最小上下文的 JSON 对象，其中包括 `event` 和 `session_dir`。

stdout 规则：

- 有效 JSON：视为结构化输出
- 非 JSON 文本：包装为 `{"additional_context": "..."}`

退出码：

| Code | Meaning |
|---|---|
| `0` | success |
| `2` | 阻止该操作 |
| other | 非阻断性的 hook 错误 |

## 结构化 Hook 输出

| Field | Meaning |
|---|---|
| `block` | 阻止该操作 |
| `updated_args` | 替换 Tool 参数 |
| `updated_result` | 替换 Tool 结果 |
| `additional_context` | 向下一轮注入额外上下文 |
| `permission_override` | 计划中的权限覆盖字段 |

## MCP

`.mcp.json` 使用标准 MCP 格式。该集成目前仍处于计划阶段，还未作为稳定行为完整文档化。
