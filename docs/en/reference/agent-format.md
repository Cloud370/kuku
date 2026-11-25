# Agent preset format

An Agent preset defines defaults for creating a Session Agent. The preset name selects the definition; a Session-local address identifies the Agent for later messages. Multiple Agents can share a preset while keeping independent contexts.

## Location and shape

Put user presets in `~/.kuku/agents/NAME.md` and project presets in `<workspace>/.kuku/agents/NAME.md` (project wins on name conflict; auto-discovery can add extra directories). [Config](config.md)

```markdown
---
name: my-reviewer
description: Review tests and cite the relevant files
connection: work
model: your-model-id
tools: [find_files, read_file, search_text]
---

Identify one missing test case and explain the expected behavior.
```

## Fields

| Field | Meaning |
|---|---|
| Markdown body | Default Agent instructions |
| `name` | Preset name (defaults to the filename stem) |
| `description` | Optional catalog description |
| `connection`, `model` | Optional pair of a configured connection name and exact model ID. Omit both to use the Session's default selection. |
| `tools` | Explicit tool names (`[]` gives no tools) |
| `tool_profile` | Fallback tool set when `tools` is omitted: `read` (default), `read_write`, or `none` |

The loader also parses `max_turns` (default `10`) as metadata. Execution budgets come from the Session's runtime limits. [Preset resolution](../../../crates/kuku/src/session/runtime/selection.rs) · [SDK limits](sdk.md#limits-and-accounting)

## Using a preset

The model can call `create_agent` with `{"address":"reviewer","preset":"my-reviewer"}` and then `send_message` with `{"to":"reviewer","message":"Review the new tests"}`. Direct SDK callers set `AgentConfig.preset`. Explicit instructions, model pair, and tool selection override preset defaults. [Session coordination tools](tools.md#session-coordination-tools)
