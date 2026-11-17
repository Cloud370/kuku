# Agents and Skills

An Agent is an addressable participant in one Session. A Skill supplies instructions or resources to the Agent that loads it. Choose a Skill when the same Agent should follow a workflow. Choose another Agent when a task needs its own context, model or execution state.

## Working with another Agent

The CLI starts with `main` by default. Create another Agent at a flat address such as `reviewer`, optionally selecting a preset for its initial instructions, tier and tools. Give it the material it needs for the task. Reusing the address continues that Agent's own history. Separate Agents can run at the same time in one workspace.

The model-facing `create_agent` tool registers the recipient. `send_message` delivers input to an existing address and returns a receipt. The recipient's output is available later through `list_agents` or Session inspection. The CLI's `kuku agents list` shows discoverable presets, while `kuku list SESSION_ID` shows the Agents that actually exist in that Session. [Use Agents](../guides/use-agents.md) · [Coordination tools](../reference/tools.md#session-coordination-tools)

## Loading a Skill

`list_skills` and `search_skills` discover Skills. `use_skill` loads one into the current Agent's context. A CLI prompt that starts with a recognized `/skill-name` can load it on the first request. The same tool authorization rules apply to actions requested after loading a Skill. [Use Skills](../guides/use-skills.md) · [Permissions](permissions.md)
