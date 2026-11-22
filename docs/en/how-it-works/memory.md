# Memory

Memory stores durable guidance, facts, and pointers in plain Markdown:

- **Global**: `$KUKU_HOME/memory.md`
- **Project**: `$KUKU_HOME/p/<workspace>/memory.md` (scoped to one workspace)

## Sections

| Section | Purpose |
|---|---|
| `how_to_work` | Collaboration preferences and working rules |
| `what_is_true` | Long-lived facts that affect decisions |
| `where_to_look` | Pointers to key files or resources |

## When an Agent sees edits

A fresh Agent context loads global Memory followed by project Memory. Editing either file takes effect for new contexts, while an existing Agent keeps the snapshot already admitted into its context segment (even across reopens). If an active Agent needs to know about a change right away, tell it directly in a message.

`remember_memory` and `forget_memory` edit these files and record tool results in the Session; you can also edit the Markdown files directly. [Manage Memory](../guides/manage-memory.md) · [Prompt assembly](../architecture/prompt-assembly.md)
