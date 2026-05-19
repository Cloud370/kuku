# Prompt

How kuku assembles prompts for the model. The goal is four things at once: SDK-first, clean boundaries, cache-friendly, low format noise.

## Layers

### system prompt

Stable runtime contract. Contains identity, hard rules, and working style. Does NOT contain workspace, date, memory, task-specific content, or host-specific behavior. Same across all projects — maximum cache reuse.

### project_context

Behavior framework. Rendered into messages[0]. Contains:

- Project instructions (`AGENTS.md` / `CLAUDE.md`)
- Execution context (workspace, platform, current date)
- Memory (global then project)
- Model tiers (available tier names and purposes)

Same project, same files → cache hit across sessions.

### tool_guidance

How to use tools: when to pick which tool, when to batch, when to serialize. Separate from system prompt to keep both small.

### runtime_context

Dynamic content per turn. Rendered into the current user turn, before the human input. Contains:

- Agent catalog (available subagents)
- Context drift notices (file-backed context changes)

Separating this from project_context keeps the stable prefix cacheable while dynamic catalogs change per turn.

Assembly order is defined in [architecture.md](core/architecture.md#context-assembly-a2b).

## Cache impact

```text
system                     global cache — identical across all projects
messages[0]                project-level cache — hit if instructions/memory unchanged
messages[1]                global cache — identical across all projects
messages[2..N-1]           session cache — hit for turns already seen
messages[N]                miss — runtime_context changes per turn
```

`tools` are at position 0 in provider-native requests. Changing the tool list invalidates all caches.

## Tags

Prompt blocks use `kuku_*` namespaced tags — not generic names that risk collisions with provider sanitization:

| `<kuku_identity>` `<kuku_hard_rules>` `<kuku_working_style>` | system prompt sections |
| `<kuku_project_context>` `<kuku_project_instructions>` `<kuku_execution_context>` | behavior framework |
| `<kuku_memory>` `<kuku_global_memory>` `<kuku_project_memory>` | memory blocks |
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
├── runtime-context.md     dynamic content wrapper
└── notice-context-drift.md  drift notice template
```

Only `project-context.md` and `runtime-context.md` use `{{placeholder}}` template variables. `system.md` and `tool-guidance.md` are used verbatim. `notice-context-drift.md` is loaded separately by the notice module, not through the prompt catalog. Rust code owns the catalog, typed inputs, and rendering — not the prompt text itself.
