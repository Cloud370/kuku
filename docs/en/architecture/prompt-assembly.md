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

### System prompt

The stable runtime contract. It carries identity, hard rules, and working style. It does not carry workspace-specific state.

### Prelude messages

The first prelude messages carry reusable context:

| Position | Content |
|---|---|
| `messages[0]` | tool guidance |
| `messages[1]` | global `Memory` |
| `messages[2]` | project `Memory` |
| `messages[3]` | project context |

Project context includes project instructions, execution context, and available model tiers.

### History

Conversation history is rebuilt from `events.jsonl` for the active conversation address, after filtering rolled-back events and applying the current handoff boundary.

### Runtime context

Dynamic data for the current turn goes into the last user message before the human input. This includes:

- agent directory notices for `main`
- open conversation notices
- inbox notices
- loaded-skill notices
- pending-permission notices
- interrupted-turn notices
- context-drift notices

## Assembly Order

```text
system prompt
messages[0]    tool_guidance
messages[1]    global_memory
messages[2]    project_memory
messages[3]    project_context
messages[4..]  replayed history for one conversation
last user turn runtime_context + human input
```

## Cache Behavior

Stable content stays in the prelude so provider-side prompt caching can reuse it across turns and conversations. Dynamic runtime data stays in the last user message so it can change without invalidating the whole prefix.

## Asset Ownership

Prompt text lives in `crates/kuku/prompts/`. The `prompt/` module owns asset loading and rendering. The `context/` and `conversation/` modules decide how one conversation's history, rollback state, and notices become a request.

See [Module Contracts](module-contracts.md) for ownership boundaries. For reader-facing behavior, see [Sessions](../how-it-works/sessions.md) and [Agents and Skills](../how-it-works/agents-and-skills.md).
