# Use Agents

## Give a task to a second Agent

Start a Session and copy its ID from the output. Use the same workspace for the following commands and replace `SESSION_ID` with that ID.

```bash
kuku run "Inspect this project and summarize the test setup"
kuku run --session SESSION_ID --agent reviewer "Read the test files and identify a missing case"
kuku list SESSION_ID
kuku show SESSION_ID --agent reviewer
```

The second command creates `reviewer` if needed. Another `kuku run` to the same Session and address continues its history. Supply the relevant files or findings when delegating work. A new Agent starts with its own context.

To share a retained log or result with an existing Agent, ask the sender to save it under a name and include that name in the `values` argument of `send_message`. The received task includes the exact saved reference. [Named Session values](../reference/tools.md#named-session-values)

## Reuse a preset

Put a Markdown preset in `<workspace>/.kuku/agents/` for that project or `~/.kuku/agents/` for personal use. Project definitions take precedence over user definitions of the same name. Inspect the catalog with `kuku agents list` and `kuku agents show NAME`. [Agent format](../reference/agent-format.md)

During a run, a model can use `create_agent` with a new address and the selected `preset`, then `send_message` with the task. `list_agents` shows the recipient's state and latest settled output. Use `--no-agents` to disable those model-facing coordination tools for the run. [Tools](../reference/tools.md) · [Manage Sessions](manage-sessions.md)
