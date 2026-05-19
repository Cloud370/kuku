# Memory

Memory is long-lived background context stored in markdown files. It is not a database, an index, or a retrieval system.

## Two layers

| Scope | Path | Loaded |
|-------|------|--------|
| Global | `$KUKU_HOME/memory.md` | Every session |
| Project | `$KUKU_HOME/p/<workspace>/memory.md` | Sessions under that workspace |

Global memory loads first, then project memory. Both are injected into `project_context` during context rebuild.

## Format

`memory.md` is plain markdown. Three fixed sections, each containing natural-language bullets:

```markdown
# memory

## how_to_work
- Prefer short replies, no trailing summaries

## what_is_true
- User knows Go well, new to React
- Merge freeze started 2026-03-05 for mobile release

## where_to_look
- Pipeline bugs tracked in Linear project "INGEST"
```

| Section | What it stores |
|---------|---------------|
| `how_to_work` | User preferences, collaboration style, behavioral feedback |
| `what_is_true` | Long-lived facts, background, decisions, constraints |
| `where_to_look` | Pointers to external resources |

No ids, timestamps, frontmatter, or machine schema.

## Tools

Memory is edited through two restricted tools — not through general file tools:

| Tool | Args | Effect |
|------|------|--------|
| `memory.remember` | `scope`, `kind`, `text` | Append one bullet to the matching section |
| `memory.forget` | `scope`, `text` | Remove one bullet by exact text match. Fails if zero or more than one match. |

`scope`: `global` or `project`. `kind`: `how_to_work`, `what_is_true`, or `where_to_look`.

To update a memory: call `memory.forget` then `memory.remember` in the same tool batch. Tools execute in order; forget runs before remember.

Memory tools write to `events.jsonl` like any other tool. The new memory snapshot takes effect on the next turn's context rebuild — the current turn sees the change through the tool result.

## What not to save

- Temporary task state, current session summaries
- Project structure derivable from code or git
- Debugging traces, one-off fixes
- Rules already in `AGENTS.md` / `CLAUDE.md`
- Secrets, tokens, credentials
- Uncertain inferences

When in doubt, ask the user before writing.

## Writing standards

Memory is agent-maintained, not auto-collected. The agent writes memory through `memory.remember` / `memory.forget`, following the guidance in its system prompt and tool-guidance.

### What deserves memory

Memory should capture behavioral guidance that changes how the agent works across sessions. A statement of fact only belongs in memory when it affects decisions.

| Remember | Don't remember |
|----------|---------------|
| "Reply in Chinese" | "User is Chinese" |
| "Explain React concepts from scratch — user is new to it" | "User knows React basics" |
| "Pipeline bugs in Linear INGEST" | "We use Linear for bug tracking" |

### Writing rules

- One short sentence per bullet. Distill, don't transcribe.
- If a bullet overlaps with an existing one, update the old entry — don't add a second.
- If a bullet no longer applies, remove it.
- Rules already in `AGENTS.md` / `CLAUDE.md` should not be duplicated in memory.

### User visibility

The user can read `memory.md` at any time. The agent tells the user what it remembered or changed in one short sentence after memory operations. Memory edits are never invisible.

## Context drift

Memory files can change between turns — edited by the agent, the user, or an external process. kuku detects these changes by comparing file hashes against the last acknowledged snapshot.

When drift is detected, a `<kuku_system_notice>` is injected into `runtime_context`. The notice signals that something changed — it does not re-inject the file content. The model should re-read the file if it needs the current state.

Tracked files are project instructions, global/project memory, and successful full-file `read_file` snapshots. `find_files`, `search_text`, and partial reads do not create drift baselines.

A successful full-file `read_file` or tool-based write updates the acknowledged baseline for that file, clearing the drift flag for subsequent turns. Partial reads (with `offset`/`limit`) do not update the baseline.
