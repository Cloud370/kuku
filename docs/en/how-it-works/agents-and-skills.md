# Agents and Skills

- Use a **Skill** when the *same* Agent should follow a reusable workflow.
- Use another **Agent** when a task needs its own context, model, or execution state.

## Working with another Agent

The CLI defaults to the `main` Agent. Create another Agent at any flat address such as `reviewer`, optionally from a preset for its instructions, model pair, and tools. Send it the context it needs. Reusing the same address continues that Agent's history. Multiple Agents can run concurrently in the same workspace.

Inside a run, `create_agent` registers a recipient, `send_message` delivers input and returns a receipt, and `list_agents` inspects status and latest settled output. From the CLI, `kuku agents list` shows available presets, while `kuku list SESSION_ID` shows Agents that actually exist in that Session. [Use Agents](../guides/use-agents.md) · [Coordination tools](../reference/tools.md#session-coordination-tools)

## Loading a Skill

`list_skills` and `search_skills` browse the catalog, and `use_skill` loads a Skill into the current Agent's context. A positional CLI prompt beginning with `/skill:NAME` loads the selected Skill for the first request. The Agent's existing permission mode applies to every operation. [Use Skills](../guides/use-skills.md) · [Tool permissions](permissions.md)
