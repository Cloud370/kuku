# Agent Format

## Mental Model

An agent file defines a contact card. The runtime binds that contact card to one or more conversation addresses.

- the contact card comes from the agent file
- the conversation address comes from `agent(to, message, tier?)`
- reusing the same address means continue the same thread

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
| `name` | no | filename stem | Contact card identifier |
| `description` | yes | none | When to use this agent |
| `model` | no | `balanced` | Default tier for first bind |
| `tools` | no | `tool_profile` default read profile | Allowed tools |
| `max_turns` | no | `10` | Maximum turns for one delegated conversation |

Notes:

- Omitting `tools` uses the agent's `tool_profile`; the default read profile allows `find_files`, `read_file`, `search_text`, `fetch_url`, and `fetch_web`.
- `tools: []` means no tools.
- The runtime hashes the effective identity into a `binding_id`.

## How Binding Works

When the model calls `agent(to, message, tier?)`:

- `to` must be a valid conversation address
- `main` is reserved and rejected
- the root segment of `to` must match a discovered agent name
- `tier` overrides the agent file's `model` only on first bind
- once a conversation address exists, passing `tier` again is rejected
- once a conversation address has `max_turns` completed turns, continuing it is rejected before a new nested run starts
- once a conversation address exists, the binding identity must still match

Example:

- `agent(to="review", message="check auth")` opens or continues `review`
- `agent(to="review/api", message="focus on handlers")` opens or continues `review/api`

Both addresses use the `review` contact card. They are different conversations.

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
