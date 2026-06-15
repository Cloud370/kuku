# Prompt Assembly

This page describes how kuku builds a model request.

## Goal

Prompt assembly tries to keep four things true at once:

- a stable runtime contract
- clear ownership between static and dynamic context
- good cache behavior
- low format noise

## Canonical Inputs

Prompt assembly is conversation-scoped, not based on separate delegated session trees.

- the session ledger is global history storage
- the active conversation address selects the replay slice
- the bound agent identity shapes tools and notices for that conversation

## Layers

### System prompt (`catalog.system.text`)

The stable runtime contract. It carries identity, hard rules, and working style.
It does not carry workspace-specific state.

### Prelude messages (snapshot layers 2–6)

The prelude is a frozen snapshot of reusable context. It is captured once per turn
and restored from `PromptSnapshot` events on later turns. Layers:

| Layer | Content | Template |
|---|---|---|
| 2 | project policy | `blocks/project-policy.md` + rendered project instructions and model tiers |
| 3 | agent identity | `input.agent_instructions` |
| 4 | agent catalog + loaded skills | injected by caller via prelude push |
| 5 | tool guidance | `blocks/tool-guidance.md` |
| 6 | memory | `blocks/memory.md` + `memory/global.md` + `memory/project.md` (gated by `enable_memory`) |

### History

Conversation history is rebuilt from `events.jsonl` for the active conversation address,
after filtering rolled-back events and applying the current handoff boundary.

### Per-turn content

Dynamic data for the current turn is injected into the last user message before the
human input. This is NOT part of the frozen snapshot:

- runtime context (agent catalog, notices, skill catalog) wrapped in `runtime/context.md`
- response contract (surface, locale, preferences) for the main conversation

Notice types that appear in runtime context: agent directory, open conversations,
inbox, loaded skills, context drift.

## Assembly Order

```text
system prompt
prelude[0]       project_policy
prelude[1]       agent_identity
prelude[2]       agent catalog + skills (injected by caller)
prelude[3..]     tool_guidance, memory*
messages[N..]    replayed history for one conversation
last user turn   runtime_context + human input
```

## Cache Behavior

Stable content stays in the prelude so provider-side prompt caching can reuse it
across turns and conversations. Dynamic runtime data stays in the last user message
so it can change without invalidating the whole prefix.

## Asset Ownership

Prompt text lives in `crates/kuku/prompts/` organized by category:

| Directory | Contents |
|---|---|
| `blocks/` | reusable template blocks (project-policy, tool-guidance, memory, notices) |
| `agents/` | agent definitions with YAML frontmatter |
| `memory/` | global and project memory templates |
| `runtime/` | runtime context, handoff context and instruction wrappers |
| `tools/` | tool-specific system prompts |

The `prompt/` module owns asset loading and rendering. The `context/` and
`conversation/` modules decide how one conversation's history, rollback state,
and notices become a request.

See [Module Contracts](module-contracts.md) for ownership boundaries. For
reader-facing behavior, see [Sessions](../how-it-works/sessions.md) and
[Agents and Skills](../how-it-works/agents-and-skills.md).
