# Prompt

How kuku assembles prompts for the model. The goal is four things at once: SDK-first, clean boundaries, cache-friendly, low format noise.

## Layers

### system prompt

Stable runtime contract. Contains identity, hard rules, and working style. Does NOT contain workspace, date, memory, task-specific content, or host-specific behavior. Same across all projects — maximum cache reuse.

### project_context

Behavior framework. Rendered into messages[3]. Contains:

- Project instructions (`AGENTS.md` / `CLAUDE.md`)
- Execution context (workspace, platform, current date)
- Model tiers (available tier names and purposes)

Same project, same files → cache hit across sessions.

### global_memory / project_memory

Memory is rendered as independent prelude messages (messages[1] and messages[2]), not embedded in project_context. Each uses a dedicated template (`global-memory.md`, `project-memory.md`) with a single `{{memory_content}}` placeholder.

Separating memory from project_context keeps each prelude message independently cacheable and prevents memory updates from invalidating the entire behavior framework prefix.

### tool_guidance

How to use tools: when to pick which tool, when to batch, when to serialize. Rendered as messages[0] (first prelude message). Same across all users and projects — maximum cache reuse.

### runtime_context

Dynamic content per turn. Rendered into the current user turn, before the human input. Contains:

- Agent catalog (available subagents)
- Context drift notices (file-backed context changes)

Separating this from project_context keeps the stable prefix cacheable while dynamic catalogs change per turn.

Assembly order is defined in [architecture.md](core/architecture.md#context-assembly-a2b).

## Cache impact

```text
system                     global cache — identical across all projects
messages[0]                global cache — tool_guidance identical across all projects
messages[1]                user cache — hit if global memory unchanged
messages[2]                project cache — hit if project memory unchanged
messages[3]                project cache — hit if instructions/env unchanged
messages[4..N-1]           session cache — hit for turns already seen
messages[N]                miss — runtime_context changes per turn
```

Prelude messages[0..3] are frozen on the first turn. On subsequent turns they are restored from the first `model.request` event, so memory updates during a session do not invalidate the Anthropic prefix cache.

`tools` are at position 0 in provider-native requests. Changing the tool list invalidates all caches.

### Design invariant

Static content (memory, instructions, tool guidance) goes into prelude messages[0..3]. Dynamic content (notices, catalogs) goes into `runtime_context`, which is injected into the last user message. The last user message is always a cache miss — placing all dynamic content there ensures the frozen prelude prefix stays cache-stable regardless of what future features emit at runtime.

## Tags

Prompt blocks use `kuku_*` namespaced tags — not generic names that risk collisions with provider sanitization:

| `<kuku_identity>` `<kuku_hard_rules>` `<kuku_working_style>` | system prompt sections |
| `<kuku_project_context>` `<kuku_project_instructions>` `<kuku_execution_context>` | behavior framework (messages[3]) |
| `<kuku_global_memory>` | global memory block (messages[1]) |
| `<kuku_project_memory>` | project memory block (messages[2]) |
| `<kuku_models>` | available model tiers |
| `<kuku_tool_guidance>` | tool usage guidance |
| `<kuku_runtime_context>` | dynamic content wrapper |
| `<kuku_agent_catalog version="N">` | subagent catalog (versioned by registry hash) |
| `<kuku_system_notice>` | runtime-injected signals (drift, registry changes) |

Human input is never wrapped in tags. Only runtime-authored content uses `kuku_*` blocks.

## Asset locations

```text
crates/kuku/prompts/
├── system.md              system prompt
├── project-context.md     behavior framework template
├── tool-guidance.md       tool usage guidance
├── global-memory.md       global memory template
├── project-memory.md      project memory template
├── runtime-context.md     dynamic content wrapper
└── notice-context-drift.md  drift notice template
```

`project-context.md` and `runtime-context.md` use `{{placeholder}}` template variables. `global-memory.md` and `project-memory.md` use a single `{{memory_content}}` placeholder. `system.md` and `tool-guidance.md` are used verbatim. `notice-context-drift.md` is loaded separately by the notice module, not through the prompt catalog. Rust code owns the catalog, typed inputs, and rendering — not the prompt text itself.
