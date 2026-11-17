# Agent 预设格式

Agent 文件提供创建 Session Agent 时可选的默认值。预设名称用来选择定义，Session 内的地址用来投递后续消息。两个 Agent 可以使用同一预设，并各自保留独立上下文。

## 文件与位置

个人预设放在 `~/.kuku/agents/NAME.md`，项目预设放在 `<workspace>/.kuku/agents/NAME.md`。同名时优先选择项目定义。启用自动发现还可以读取其他位置。[配置](config.md)

```markdown
---
name: my-reviewer
description: Review tests and cite the relevant files
tier: balanced
tools: [find_files, read_file, search_text]
---

Identify one missing test case and explain the expected behavior.
```

## 字段

| 字段 | 含义 |
|---|---|
| Markdown 正文 | Agent 的默认行为指令。 |
| `name` | 预设名称，默认使用不含扩展名的文件名。 |
| `description` | 可选的目录说明。 |
| `tier` | 默认模型档，加载器也接受 `model`，缺省时使用 `balanced`。 |
| `tools` | 显式指定工具名称。空列表选用零个工具。 |
| `tool_profile` | 省略 `tools` 时使用的工具集，可选 `read`、`read_write`、`none`，默认是 `read`。 |

加载器还会解析 `max_turns`，默认值为 10。公开 Session 从预设中应用指令、模型档和工具，执行预算由 Session 运行时限额控制。[预设解析](../../../crates/kuku/src/session/runtime/selection.rs) · [SDK 限额](sdk.md#限额与用量)

## 使用预设

模型可以调用 `create_agent` 并传入 `{"address":"reviewer","preset":"my-reviewer"}`，随后调用 `send_message` 并传入 `{"to":"reviewer","message":"Review the new tests"}`。直接调用 SDK 时设置 `AgentConfig.preset`。显式指定的指令、模型档和工具选择分别覆盖相应的预设默认值。[Session 协作工具](tools.md#session-协作工具)
