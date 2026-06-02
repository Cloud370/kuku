# Use Agents

Agents are named subagents that run in child sessions.

## Add an Agent

Place agent files in one of these locations:

- `~/.kuku/agents/`
- `<workspace>/.kuku/agents/`

These are the conventional locations. With auto-discovery enabled, kuku also scans other user and project dot-directories for both `agents/` and `agent/`, such as `.claude/agents` and `.opencode/agent`.

Project agents override user agents with the same name.

The file format is defined in [Agent Format](../reference/agent-format.md).

## Check Discovery

```bash
kuku agents list
kuku agents show <name>
```

If an agent does not appear, review your `[discovery]` settings in [Config](../reference/config.md).

## Use Agents in a Run

When the agent tool is enabled, kuku can delegate part of the work to discovered agents.

To disable agent delegation for one run:

```bash
kuku run --no-agents "task"
```

## Choose User vs Project Scope

- Use `~/.kuku/agents/` for personal reusable agents.
- Use `<workspace>/.kuku/agents/` when the agent belongs to one repository.

## Related Pages

- [Skill Format](../reference/skill-format.md)
- [Tools](../reference/tools.md)
