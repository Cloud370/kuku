# Agent Format

## Locations

- User scope: `~/.kuku/agents/<name>.md`
- Project scope: `<workspace>/.kuku/agents/<name>.md`

Project agents override user agents with the same name.

## File Shape

Agent files are Markdown with YAML frontmatter.

```markdown
---
name: my-reviewer
description: Pre-commit code review with file and line evidence
model: balanced
tools: [find_files, read_file, search_text]
max_turns: 10
---

You are a thorough code reviewer.
```

## Fields

| Field | Required | Default | Meaning |
|---|---|---|---|
| `name` | no | filename stem | Agent identifier |
| `description` | yes | none | When to use this agent |
| `model` | no | `balanced` | Tier name |
| `tools` | no | default built-in registry | Allowed tools |
| `max_turns` | no | `10` | Maximum child-session turns |

Notes:

- Omitting `tools` gives the child the default built-in tool registry without `agent` or `use_skill`.
- `tools: []` means no tools.

## Accepted Tool Names

Current documented tool names include:

- `find_files`
- `read_file`
- `search_text`
- `edit_file`
- `write_file`
- `run_command`
- `remember_memory`
- `forget_memory`
- `fetch_url`
- `fetch_web`
- `query_session`

## Discovery Notes

The loader accepts known field aliases such as `maxTurns` for `max_turns` and `allowedTools` for `tools`. Unknown frontmatter fields are preserved as metadata.
