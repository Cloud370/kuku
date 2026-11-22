# How it works

Mental model and runtime behavior behind kuku.

- [Session files and the Rust SDK](why-kuku.md) — journal inspection, resumable Sessions, and SDK embedding
- [File-native execution](file-native-model.md) — on-disk Session logs, context snapshots, and recovery
- [Agent loop](agent-loop.md) — Turns, Steps, tool batches, waiting, and handoff
- [Sessions](sessions.md) — delivery, Background Jobs, journal sealing, compaction, rewind, and cleanup
- [Performance](performance.md) — measured install, idle memory, reopen latency, and ARM64 numbers
- [Memory](memory.md) — global and project Memory files and when Agents see edits
- [Agents and Skills](agents-and-skills.md) — when to load a Skill vs. delegate to another Agent
- [Tool permissions](permissions.md) — `ToolAuthorizer`, deferred approval, `run_command`, and `run_js`

New to the internals? Start with [Session files and the Rust SDK](why-kuku.md), then [File-native execution](file-native-model.md) and [Agent loop](agent-loop.md).
