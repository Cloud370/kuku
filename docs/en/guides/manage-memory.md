# Manage Memory

Memory is durable context stored as plain Markdown on disk and injected into future Sessions.

## Two scopes

- **Global**: `$KUKU_HOME/memory.md` — applies to all Sessions.
- **Project**: `$KUKU_HOME/p/<workspace>/memory.md` — applies only to one workspace.

You can read or edit either file directly as plain text at any time. [File layout](../reference/file-layout.md)

## Letting the Agent update Memory

The runtime exposes two built-in tools:

- `remember_memory`
- `forget_memory`

Their exact arguments are documented in [Tools](../reference/tools.md).

## Keep Memory concise

Put stable preferences, durable project facts, and pointers that actually change future decisions in Memory. Keep these out:

- one-off task notes
- Session transcripts
- secrets or credentials
- rules already stated in `AGENTS.md` or `CLAUDE.md`

See also [How Memory works](../how-it-works/memory.md) and [Glossary](../reference/glossary.md).
