# File-Native Model

kuku treats files as the source of runtime truth.

## The model

- A `Session` is a directory under `$KUKU_HOME`.
- `events.jsonl` is the durable event log for that session.
- Global and project `Memory` live in `memory.md` files.
- Project instructions come from files such as `AGENTS.md` and `CLAUDE.md`.
- Permission state and extension packages are file-backed.

There is no separate database that defines the run. If a fact matters, it must be recoverable from files.

## What gets rebuilt

Before each model call, kuku rebuilds the request context from:

1. Prompt assets.
2. Project instructions.
3. Global and project `Memory`.
4. Prior persisted events.
5. Current runtime notices and catalogs.

This keeps the execution model stable across hosts and across process restarts.

## What stays derived

Some views exist for convenience, but they are not separate state:

- rendered transcripts
- session summaries
- inspection output
- UI event streams

Those views come from files that already exist, mainly `events.jsonl` and the current workspace files.

## Recovery rule

Only appended events are trusted. If a process stops mid-turn, kuku resumes from the last confirmed facts in the event log instead of guessing what happened.

## Relationship to hosts

The SDK owns the file-backed runtime model. Host apps own presentation, transport, and user interaction. The host can be a terminal app, a server, or another interface, but the persisted session model stays the same.

See [Sessions](sessions.md) for the session directory model, [Agent Loop](agent-loop.md) for turn execution, and [Prompt Assembly](../architecture/prompt-assembly.md) for the maintainer view of context rebuild.
