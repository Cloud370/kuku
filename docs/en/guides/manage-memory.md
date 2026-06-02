# Manage Memory

Memory is long-lived context stored on disk.

## Know the Two Scopes

- Global memory applies to all sessions.
- Project memory applies only to one workspace.

Paths are listed in [File Layout](../reference/file-layout.md).

## Let the Agent Update Memory

The runtime exposes two dedicated tools:

- `remember_memory`
- `forget_memory`

Their exact arguments are defined in [Tools](../reference/tools.md).

## Review Memory Files

Read the current memory files directly when you want to inspect or edit them as text:

- global: `$KUKU_HOME/memory.md`
- project: `$KUKU_HOME/p/<workspace>/memory.md`

## Keep Memory Small

Good memory entries are stable guidance, durable facts, and pointers that change future decisions.

Do not use memory for:

- temporary task notes
- session transcripts
- secrets
- facts already enforced by `AGENTS.md` or `CLAUDE.md`

## Related Pages

- [File Layout](../reference/file-layout.md)
- [Glossary](../reference/glossary.md)
