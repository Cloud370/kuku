# Use Agents

Agent 是在子 Session 中运行的命名 subagent。

## Add an Agent

把 agent 文件放到以下位置之一：

- `~/.kuku/agents/`
- `<workspace>/.kuku/agents/`

这些是约定位置。启用 auto-discovery 时，kuku 也会扫描其他用户级和项目级 dot-directory 中的 `agents/` 和 `agent/`，例如 `.claude/agents` 与 `.opencode/agent`。

同名情况下，项目级 agent 会覆盖用户级 agent。

文件格式定义见 [Agent Format](../reference/agent-format.md)。

## Check Discovery

```bash
kuku agents list
kuku agents show <name>
```

如果某个 agent 没有出现，请检查 [Config](../reference/config.md) 中的 `[discovery]` 设置。

## Use Agents in a Run

当 agent tool 启用时，kuku 可以把部分工作委派给已发现的 Agent。

如果要在某一次运行中禁用 agent delegation：

```bash
kuku run --no-agents "task"
```

## Choose User vs Project Scope

- `~/.kuku/agents/` 适合个人可复用的 agent。
- `<workspace>/.kuku/agents/` 适合只属于某个仓库的 agent。

## Related Pages

- [Skill Format](../reference/skill-format.md)
- [Tools](../reference/tools.md)
