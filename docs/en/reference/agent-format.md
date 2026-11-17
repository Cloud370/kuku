# Agent preset format

An Agent file supplies defaults used when creating a Session Agent. A preset name selects the definition. A Session-local address selects the Agent for later messages. Two Agents can use the same preset and retain independent contexts.

## File and location

Use `~/.kuku/agents/NAME.md` for a user preset or `<workspace>/.kuku/agents/NAME.md` for a project preset. Project definitions win on a name collision. Auto-discovery can add other locations. [Configuration](config.md)

```markdown
---
name: my-reviewer
description: Review tests and cite the relevant files
tier: balanced
tools: [find_files, read_file, search_text]
---

Identify one missing test case and explain the expected behavior.
```

## Fields

| Field | Meaning |
|---|---|
| Markdown body | Default Agent instructions. |
| `name` | Preset name. Defaults to the filename stem. |
| `description` | Optional catalog description. |
| `tier` | Default configured tier. The loader also accepts `model` and defaults to `balanced`. |
| `tools` | Explicit tool names. An empty list selects no tools. |
| `tool_profile` | Tools to use when `tools` is omitted. Accepts `read`, `read_write` and `none`. The default is `read`. |

The loader also parses `max_turns`, defaulting to 10. Public Session preset resolution applies instructions, tier and tools. Execution budgets come from the Session's runtime limits. [Preset resolution](../../../crates/kuku/src/session/runtime/selection.rs) · [SDK limits](sdk.md#limits-and-accounting)

## Select the preset

The model can call `create_agent` with `{"address":"reviewer","preset":"my-reviewer"}` and then `send_message` with `{"to":"reviewer","message":"Review the new tests"}`. Direct SDK callers set `AgentConfig.preset`. Explicit instructions, tier and tool selection override the corresponding preset defaults. [Session coordination tools](tools.md#session-coordination-tools)
