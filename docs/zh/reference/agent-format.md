# Agent Format

## Mental Model

agent 文件定义的是联系人卡片。运行时会把这张联系人卡片绑定到一个或多个 conversation address。

- 联系人卡片来自 agent 文件
- conversation address 来自 `agent(to, message, tier?)`
- 复用相同 address 就表示继续同一线程

## Locations

- User scope: `~/.kuku/agents/<name>.md`
- Project scope: `<workspace>/.kuku/agents/<name>.md`

同名时，项目级 agent 会覆盖用户级 agent。

## File Shape

agent 文件是带 YAML frontmatter 的 Markdown。

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

## Fields

| Field | Required | Default | Meaning |
|---|---|---|---|
| `name` | no | filename stem | 联系人卡片标识符 |
| `description` | yes | none | 何时使用这个 agent |
| `model` | no | `balanced` | 首次绑定时的默认 tier |
| `tools` | no | `tool_profile` 的默认 read profile | 允许的工具 |
| `max_turns` | no | `10` | 单个委派 conversation 的最大 turns |

说明：

- 省略 `tools` 时，会使用该 agent 的 `tool_profile`；默认 read profile 允许 `find_files`、`read_file`、`search_text`、`fetch_url`、`fetch_web`。
- `tools: []` 表示没有工具。
- 运行时会把有效身份哈希成 `binding_id`。

## How Binding Works

当模型调用 `agent(to, message, tier?)` 时：

- `to` 必须是有效 conversation address
- `main` 是保留名，会被拒绝
- `to` 的根 segment 必须匹配已发现的 agent 名称
- `tier` 只会在首次绑定时覆盖 agent 文件中的 `model`
- 一旦 conversation address 已存在，再传 `tier` 会被拒绝
- 一旦 conversation address 已有 `max_turns` 个 completed turns，继续该 address 会在启动新的嵌套 run 前被拒绝
- 一旦 conversation address 已存在，绑定身份仍必须匹配

例如：

- `agent(to="review", message="check auth")` 会打开或继续 `review`
- `agent(to="review/api", message="focus on handlers")` 会打开或继续 `review/api`

这两个 address 都使用 `review` 这张 contact card，但它们是两个不同 conversation。

## Accepted Tool Names

当前文档列出的工具名包括：

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

## Discovery Notes

loader 接受一些已知字段别名，例如 `maxTurns` 对应 `max_turns`，`allowedTools` 对应 `tools`。未知 frontmatter 字段会作为 metadata 保留。
