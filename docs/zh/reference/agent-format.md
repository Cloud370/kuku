# Agent Format

## 位置

- 用户级：`~/.kuku/agents/<name>.md`
- 项目级：`<workspace>/.kuku/agents/<name>.md`

同名时，项目级 Agent 会覆盖用户级 Agent。

## 文件形状

Agent 文件是带 YAML frontmatter 的 Markdown。

```markdown
---
name: my-reviewer
description: Pre-commit code review with file and line evidence
model: balanced
tools: [find_files, read_file, search_text]
max_turns: 10
---

You are a thorough code reviewer.
```

## 字段

| Field | Required | Default | Meaning |
|---|---|---|---|
| `name` | no | filename stem | Agent 标识符 |
| `description` | yes | none | 何时使用这个 Agent |
| `model` | no | `balanced` | tier 名称 |
| `tools` | no | default built-in registry | 允许使用的 Tool |
| `max_turns` | no | `10` | 子 Session 的最大轮数 |

说明：

- 省略 `tools` 时，子 Session 会得到默认的 built-in Tool registry，但不包含 `agent` 和 `use_skill`。
- `tools: []` 表示不允许任何 Tool。

## 接受的 Tool 名称

当前文档中的 Tool 名称包括：

- `find_files`
- `read_file`
- `search_text`
- `edit_file`
- `write_file`
- `run_command`
- `remember_memory`
- `forget_memory`
- `fetch_url`
- `fetch_web`
- `query_session`

## 发现说明

加载器接受已知字段别名，例如用 `maxTurns` 表示 `max_turns`，用 `allowedTools` 表示 `tools`。未知的 frontmatter 字段会作为元数据保留。
