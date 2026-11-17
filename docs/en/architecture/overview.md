# Architecture Overview

This section is for maintainers. It describes crate boundaries and runtime ownership, not end-user behavior.

## High-level split

- `crates/kuku/` is the SDK.
- `crates/kuku-cli/` provides CLI command implementations.
- `crates/kuku-server/` provides the HTTP server.
- `apps/kuku/` builds the unified release binary.

The SDK owns runtime facts, event persistence, context rebuild, provider calls, tool dispatch, and permission decisions. Hosts own presentation, transport, and interaction.

## SDK shape

```text
crates/kuku/src/
|- query/
|- context/
|- provider/
|- tool/
|- permission/
|- session/
|- event/
|- prompt/
|- skill/
|- plugin/
|- config/
|- conversation/
|- agent/
|- notice/
|- util/
|- wire.rs
`- error.rs
```

## Runtime invariants

- Session truth is on disk.
- `events.jsonl` is append-only.
- The SDK prepares each Step against the Agent's committed context prefix and admits new messages incrementally.
- Provider adapters convert protocol formats but do not own runtime state.
- Permission checks happen in the runtime, not in host UI.

## Data layout

The default data root is `$KUKU_HOME` or `~/.kuku`. It stores configuration, Memory, project policy and Session journals. Project instruction files and the files modified by tools live in the workspace. A Session can also use an explicit data root. [File layout](../reference/file-layout.md)

## Reading path

Start with [Host Apps](host-apps.md) if you need the user-facing host boundary, [Prompt Assembly](prompt-assembly.md) if you need request construction, and [Module Contracts](module-contracts.md) if you need internal ownership rules.

For public behavior, use [How It Works](../how-it-works/index.md) instead of this section.
