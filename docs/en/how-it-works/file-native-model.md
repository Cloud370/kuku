# File-native execution

Every Session is backed by an `events.jsonl` journal and a directory of retained command outputs, while tools read and modify project files directly in your workspace. By default the local service stores Sessions under `$KUKU_HOME` (overridable via `--data-dir`), and direct SDK embedders choose their own storage root. [File layout](../reference/file-layout.md)

## What an Agent remembers

When an Agent starts a fresh context, kuku assembles the selected Prompt assets, Agent instructions, project instructions (`AGENTS.md` / `CLAUDE.md`), and Memory, and records exactly what the model received. Subsequent Steps reuse that committed prefix and append only newly admitted inputs and tool results. Editing `AGENTS.md` or `memory.md` later does not break reconstruction of earlier turns. Two Agents working in the same directory still keep completely separate contexts. [Prompt assembly](../architecture/prompt-assembly.md)

## Reading, ownership, and recovery

Clients read committed events by cursor or fetch an Agent's latest settled snapshot; live stream deltas only show in-flight progress. If a client disconnects, everything committed stays in the Session.

Reopening repairs any partial tail line in `events.jsonl` and restores pending questions or permission requests. If execution was interrupted *after* an external side effect was dispatched, that action's outcome is marked unknown so you or the Agent can check before retrying.

The HTTP service (including the managed local service used by `kuku` and `kuku run`) owns Session execution for its clients, so closing a terminal leaves active tasks running. Direct Rust SDK embedders own the `Session` in-process instead. [Sessions](sessions.md) · [Run the server](../guides/run-the-server.md)
