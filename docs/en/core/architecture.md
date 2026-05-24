# Architecture

## Crate structure

One crate. `kuku` is the SDK. Host apps (CLI, TUI, WebUI) live in `apps/` but are not part of the SDK.

SDK owns runtime facts, session state, context rebuild, provider adapters, tool dispatch, permission decisions, and event persistence. Host apps own presentation, command routing, layout, and user interaction.

```text
crates/kuku/src/
├── lib.rs              pub mod declarations, re-exports
├── query/              public API: Query builder, Run state machine, slot dispatch
├── context/            rebuild messages from files + events
├── provider/           model API adapters (Anthropic, OpenAI), SSE parsing
├── tool/               tool definitions, registry, dispatch, builtins, streaming output
├── permission/         runtime gate, hard guard, policy matching
├── session/            directory paths, writer lock, session list/delete, status
├── event/              event types, events.jsonl read/write, fast scan helpers
├── prompt/             prompt assets, catalog, template rendering
├── skill/              skill definitions, catalog, loader, registry
├── config.rs           tiers, providers, config.toml parsing
├── subagent/           definitions, registry, catalog, child sessions
├── notice/             context drift detection, system notices
├── wire.rs             UiEvent → NDJSON wire format serialization
└── error.rs            typed error enum
```

## Module rules

Each module has a clear boundary of what it may and may not depend on.

| Module | Owns | May depend on | Must not |
|--------|------|--------------|----------|
| `query/` | Agent loop, Run state machine, UiEvent stream | All other modules | — |
| `context/` | Message rebuild, provenance, assembly | `event/`, `prompt/`, `config/` | Provider, tool execution, permission |
| `provider/` | Protocol conversion (Anthropic, OpenAI) | Canonical messages, tool schemas, config | Session, event store, permission |
| `tool/` | Definitions, registry, dispatch, built-in tools | `event/`, `context/` (ToolSchema) | Provider protocol, slot scheduling |
| `permission/` | Gate decisions, hard guard, policy.md | `event/` | Tool execution, provider, session |
| `session/` | Path derivation, writer lock, session list/delete | `event/` (via scan helpers) | Provider, model state |
| `event/` | Event types, store append/replay, fast scan helpers | — | Provider, tools, permission |
| `prompt/` | Asset catalog, template rendering | — | Runtime decisions, session state |
| `config/` | Config parsing, validation, tiers | — | Provider, tools, session |
| `skill/` | Skill definitions, catalog, loader, registry | `prompt/` | Runtime decisions, session state |
| `subagent/` | Definitions, registry, catalog, child spawn | `tool/`, `query/`, `session/` | — |
| `notice/` | Drift detection, system notice rendering | `event/`, `prompt/` | — |
| `error.rs` | Typed error enum | — | — |

## Directory layout

All kuku data lives under `$KUKU_HOME`. Zero pollution of the working directory.

`$KUKU_HOME` comes from the environment or defaults to `~/.kuku`.

```text
$KUKU_HOME/
├── config.toml              global configuration
├── memory.md                 global memory (all projects)
├── p/<workspace-path>/       per-project data
│   ├── memory.md             project memory
│   ├── policy.md             project permission rules
│   ├── runtime/locks/         writer locks (transient)
│   └── sessions/<id>/
│       ├── events.jsonl
│       └── subs/             child sessions
```

Workspace paths are canonicalized and mirrored under `p/` as path components, with the root or platform prefix removed. No hashing, no encoding. `tree $KUKU_HOME/p/` is the index.

`policy.md` is local permission state for the current project path. It is read by the permission gate, not injected into ordinary model context.

## Public API

```rust
// Simple: run to completion
let output = kuku::query("check this project").run().await?;

// Interactive: stream events, handle permissions
let mut run = kuku::query("check this project")
    .workspace(workspace)
    .tier("strong")
    .start().await?;

while let Some(event) = run.next().await? {
    match event {
        UiEvent::TextDelta { text } => print!("{text}"),
        UiEvent::PermissionRequested { request } => {
            run.decide(request.id, PermissionChoice::Once).await?;
        }
        UiEvent::Done { output, .. } => break,
        _ => {}
    }
}
```

`Query` is a typed builder — no options bag. `Run` is the handle: stream events via `next()`, respond to permissions via `decide()`, read `session_id()`.

## Context assembly (A2b)

Every model call rebuilds context from files. Nothing is cached across turns.

```text
system prompt              (identity, hard rules, working style)
messages[0]                project_context: instructions, memory, tiers, environment
messages[1]                tool_guidance
messages[2..]              history rebuilt from events.jsonl
last user turn             runtime_context (catalogs, notices) + human input
```

## Project instructions

`AGENTS.md` is the primary project instruction source. `CLAUDE.md` is compatibility input. No kuku-specific instruction file is required.

Instructions are soft constraints — they guide the model, but do not grant hard permission. Hard permission comes from the `permission gate`.

## Provider boundary

Provider adapters convert canonical messages into provider-native requests and normalize responses back. They own nothing — no session state, no tool execution, no permission decisions.

```text
Canonical messages + tools → adapter → provider-native HTTP → model
```

Built-in formats: `anthropic`, `openai-chat`, `openai-responses`.
