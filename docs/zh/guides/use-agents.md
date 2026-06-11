# Use Agents

Agent 是联系人卡片。kuku 调用它时，会在当前 Session 中打开或继续某个委派 conversation address。

## Add an Agent

把 agent 文件放在以下任一位置：

- `~/.kuku/agents/`
- `<workspace>/.kuku/agents/`

这些是约定位置。启用 auto-discovery 后，kuku 也会扫描其他用户和项目 dot-directory 中的 `agents/` 和 `agent/`，例如 `.claude/agents` 和 `.opencode/agent`。

同名时，项目级 agent 会覆盖用户级 agent。

文件格式见 [Agent Format](../reference/agent-format.md)。

## Check Discovery

```bash
kuku agents list
kuku agents show <name>
```

如果某个 agent 没有出现，请检查 [Config](../reference/config.md) 中的 `[discovery]` 设置。

## Use Agents in a Run

启用 agent Tool 时，kuku 可以把部分工作委派给已发现的 agent contacts。

要用 address 来思考：

- `review` 表示一条持续中的 review 线程
- `review/api` 表示一个以 `review` contact 为根的独立嵌套线程
- 复用 `review` 表示保持连续性

`main` address 是 Host conversation 的保留名，不能作为 agent 目标。

只有在首次打开新 address 时，传 model tier 才有意义。如果在继续已有 address 时传 tier，运行时会拒绝这次调用。

要在单次运行中禁用 agent 委派：

```bash
kuku run --no-agents "task"
```

## Choose User vs Project Scope

- `~/.kuku/agents/` 适合个人复用 contacts。
- `<workspace>/.kuku/agents/` 适合属于某个仓库的 contacts。

## Inspect Conversations

```bash
kuku list <session-id>
kuku show <session-id> --conversation review
kuku events <session-id> --conversation review
```

- `kuku list <session-id>` 列出一个 Session 内的 conversation addresses。
- `kuku show` 显示某个 conversation 的最终 transcript 输出。
- `kuku events` 显示底层账本事实，并可按 conversation 过滤。

## Related Pages

- [Tools](../reference/tools.md)
- [Manage Sessions](manage-sessions.md)
