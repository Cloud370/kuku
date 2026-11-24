# Agent 预设格式

Agent 预设为新建 Session Agent 提供默认配置。预设名用于选择定义，Session 内的地址用于后续消息投递。多个 Agent 可共用同一预设且各自保持独立上下文。

## 文件位置与格式

用户级预设放在 `~/.kuku/agents/NAME.md`，项目级预设放在 `<workspace>/.kuku/agents/NAME.md`（同名时项目级优先；开启自动发现还会扫描其他目录）。[配置](config.md)

```markdown
---
name: my-reviewer
description: Review tests and cite the relevant files
connection: work
model: your-model-id
tools: [find_files, read_file, search_text]
---

Identify one missing test case and explain the expected behavior.
```

## 字段

| 字段 | 含义 |
|---|---|
| Markdown 正文 | Agent 默认行为指令 |
| `name` | 预设名，默认取不含扩展名的文件名 |
| `description` | 可选的目录简介 |
| `connection`、`model` | 可选的已配置连接名与精确模型 ID，须成对填写。都省略时使用 Session 默认值。 |
| `tools` | 显式工具名列表（空列表表示不挂载工具） |
| `tool_profile` | 省略 `tools` 时的默认工具集：`read`（默认）、`read_write` 或 `none` |

加载器会将 `max_turns`（默认 `10`）解析为元数据。执行预算由 Session 运行时限额控制。[预设解析](../../../crates/kuku/src/session/runtime/selection.rs) · [SDK 限额](sdk.md#限额与用量)

## 使用预设

模型可调用 `create_agent`（`{"address":"reviewer","preset":"my-reviewer"}`），再调用 `send_message`（`{"to":"reviewer","message":"Review the new tests"}`）。直接调用 SDK 时设置 `AgentConfig.preset`。显式传入的指令、模型选择和工具会覆盖预设默认值。[Session 协作工具](tools.md#session-协作工具)
