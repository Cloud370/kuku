# Prompt Assembly

This page describes how kuku builds a request for the model.

## Goal

Prompt assembly tries to keep four things true at once:

- a stable runtime contract
- clear ownership between static and dynamic context
- good cache behavior
- low format noise

## Layers

### System prompt

The stable runtime contract. It carries identity, hard rules, and working style. It does not carry workspace-specific state.

### Prelude messages

The first prelude messages carry reusable context:

| Position | Content |
|----------|---------|
| `messages[0]` | tool guidance |
| `messages[1]` | global `Memory` |
| `messages[2]` | project `Memory` |
| `messages[3]` | project context |

Project context includes project instructions, execution context, and available model tiers.

### History

Conversation history is rebuilt from `events.jsonl`, after filtering rolled-back turns and applying the current handoff boundary.

### Runtime context

Dynamic data for the current turn goes into the last user message before the human input. This includes catalogs and system notices such as context drift.

## Assembly order

```text
system prompt
messages[0]    tool_guidance
messages[1]    global_memory
messages[2]    project_memory
messages[3]    project_context
messages[4..]  rebuilt history
last user turn runtime_context + human input
```

## Cache behavior

Stable content is kept in the prelude so provider-side prompt caching can reuse it across turns and sessions. Dynamic runtime data stays in the last user message so it can change without invalidating the whole prefix.

## Asset ownership

Prompt text lives in `crates/kuku/prompts/`. The `prompt/` module owns asset loading and rendering, while `context/` decides how those rendered pieces are assembled into a request.

See [Module Contracts](module-contracts.md) for the boundary between those modules. For the reader-facing explanation of the same behavior, see [File-Native Model](../how-it-works/file-native-model.md) and [Memory](../how-it-works/memory.md).
