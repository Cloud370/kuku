# Use Agents

## Delegate to a second Agent

Start a Session, copy its ID from the output, and reuse the same workspace for follow-up commands (replacing `SESSION_ID`):

```bash
kuku run "Inspect this project and summarize the test setup"
kuku run --session SESSION_ID --agent reviewer "Read the test files and identify a missing case"
kuku list SESSION_ID
kuku show SESSION_ID --agent reviewer
```

The second command creates `reviewer` on demand; later `kuku run` calls to the same Session and address continue its history. Because every new Agent starts with a clean context, pass along the relevant file paths or findings when delegating.

To hand a retained log or large output to another Agent, have the sender save it under a name and pass that name in `send_message`'s `values` argument — the recipient receives the exact saved reference with its task. [Named Session values](../reference/tools.md#named-session-values)

## Reuse Markdown presets

Save Agent presets as Markdown files in `<workspace>/.kuku/agents/` (project-scoped) or `~/.kuku/agents/` (personal). Project presets override personal presets of the same name. Inspect what is discovered with:

```bash
kuku agents list
kuku agents show NAME
```

During a run, the model can call `create_agent` with a new address and a `preset`, send work via `send_message`, and check status or latest settled output with `list_agents`. Pass `--no-agents` to turn off these multi-Agent tools for a single run. [Agent format](../reference/agent-format.md) · [Tools](../reference/tools.md) · [Manage Sessions](manage-sessions.md)
