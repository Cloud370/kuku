# Memory

Memory stores reusable instructions, facts and references in Markdown. Global Memory lives at `$KUKU_HOME/memory.md`. Project Memory lives at `$KUKU_HOME/p/<workspace>/memory.md` and applies to that workspace.

## File sections

| Section | Use |
|---|---|
| `how_to_work` | Collaboration preferences and working rules. |
| `what_is_true` | Durable facts that affect decisions. |
| `where_to_look` | Pointers to relevant files or other resources. |

## When an Agent sees a change

A fresh Agent context loads global Memory followed by project Memory. Editing either file changes what a new context can load. An existing Agent keeps the version already admitted to its context segment, including after reopening. To communicate a change to that Agent now, send it the new information explicitly.

The `remember_memory` and `forget_memory` tools edit the files and record a tool result in the Session. You can also edit the files directly. [Manage Memory](../guides/manage-memory.md) · [Prompt assembly](../architecture/prompt-assembly.md)
