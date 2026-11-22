# Agents and Skills

- Use a **Skill** when the *same* Agent should follow a reusable workflow.
- Use another **Agent** when a task needs its own context, model, or execution state.

## Working with another Agent

The CLI defaults to the `main` Agent. Create another Agent at any flat address (such as `reviewer`), optionally from a preset for its instructions, tier, and tools, and send it the context it needs. Reusing the same address continues that Agent's history. Multiple Agents can run concurrently in the same workspace.

Inside a run, `create_agent` registers a recipient, `send_message` delivers input and returns a receipt, and `list_agents` inspects status and latest settled output. From the CLI, `kuku agents list` shows available presets, while `kuku list SESSION_ID` shows Agents that actually exist in that Session. [Use Agents](../guides/use-agents.md) · [Coordination tools](../reference/tools.md#session-coordination-tools)

## Loading a Skill

`list_skills` and `search_skills` browse the catalog, and `use_skill` loads a Skill into the current Agent's context. A CLI prompt beginning with `/skill-name` loads that Skill for the first request. Everything an Agent tries to do after loading a Skill still goes through the exact same tool authorization rules. [Use Skills](../guides/use-skills.md) · [Tool permissions](permissions.md)
