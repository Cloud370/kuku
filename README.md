# kuku

kuku is a Rust SDK for file-native agent execution.

The project is intentionally SDK-first: CLI, TUI, WebUI, and package tooling are host applications over the same runtime contract, not separate owners of agent state.

## Status

This repository is being initialized from the public implementation contract. The first milestone is the core SDK crate in `crates/kuku`.

## Repository layout

```text
kuku/
├── crates/
│   └── kuku/          # Core SDK/runtime crate
├── apps/
│   └── cli/           # First host app after the SDK loop works
├── packages/          # Official packages and examples, later
├── docs/
│   ├── spec/          # Public implementation specs
│   └── plans/         # Execution plans for implementation milestones
└── README.md
```

## Core idea

The runtime writes execution facts to `events.jsonl` in a session directory. Derived views such as final output, request inspection, transcripts, and UI streams are rebuilt from files and events.
