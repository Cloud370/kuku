# Agents

Subagents are defined in Markdown files with YAML frontmatter. They are
registered at `~/.kuku/agents/<name>.md` (user) or
`<workspace>/.kuku/agents/<name>.md` (project). Project agents override
user agents with the same name.

## Format

    ---
    name: my-reviewer
    description: Pre-commit code review with file/line evidence
    model: balanced
    tools: [find_files, read_file, search_text]
    max_turns: 5
    ---

    You are a thorough code reviewer. For every finding, cite the file
    path and line number as evidence.

## Fields

| Field | Required | Default | Description |
|-------|----------|---------|-------------|
| `name` | no | filename stem | Unique agent identifier |
| `description` | yes | — | When-to-use summary for catalog |
| `model` | no | `balanced` | `strong` / `balanced` / `light` |
| `tools` | no | inherit | Tool names. Omit = inherit parent. `[]` = no tools. |
| `max_turns` | no | `5` | Maximum turns before forced stop |

## Tools

- `find_files` — browse directory trees
- `read_file` — read file content with line numbers
- `search_text` — regex search over files
- `edit_file` — precise text replacement
- `write_file` — create or overwrite files
- `run_command` — execute local commands
- `remember_memory` — append to memory.md
- `forget_memory` — remove from memory.md

## Permissions

Child sessions inherit the parent's permission mode:

- **Auto-allow** (`-y`): tools auto-allowed (hard guard still active)
- **Interactive**: permission requests bubble to the host
- **Session grants**: parent's session-scoped approvals carry over

The hard guard always applies and cannot be overridden.

## Compatibility

Agents in `.claude/agents/*.md` are auto-imported. Fields `model`, `tools`,
`maxTurns`, and `description` map to kuku equivalents. Unknown fields are
preserved in metadata.
