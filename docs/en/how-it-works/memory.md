# Memory

`Memory` is long-lived background context stored in markdown files.

It is not a database, vector index, or hidden host feature.

## Two layers

| Scope | Path | Loaded |
|-------|------|--------|
| Global | `$KUKU_HOME/memory.md` | Every session |
| Project | `$KUKU_HOME/p/<workspace>/memory.md` | Sessions in that workspace |

Global `Memory` loads before project `Memory`.

## Structure

`memory.md` uses three fixed sections:

| Section | Purpose |
|---------|---------|
| `how_to_work` | Collaboration preferences and working rules |
| `what_is_true` | Long-lived facts that affect decisions |
| `where_to_look` | Pointers to external resources |

The file stays plain markdown. No ids, timestamps, or extra schema are required.

## How it changes

The runtime exposes dedicated memory tools to append or remove bullets. Memory changes are written through the same event log as other tool results, so the next turn rebuild sees the updated file.

Users can also edit the files directly.

## What belongs in Memory

Keep `Memory` for information that should change behavior across sessions, such as:

- user preferences
- durable project constraints
- important external pointers

Do not use it for:

- temporary task state
- facts already defined in project instructions
- secrets or credentials
- uncertain guesses

## Drift notices

If a tracked file changes between turns, kuku injects a system notice into runtime context. The notice says that file-backed context changed; it does not automatically reinsert the new file contents.

Tracked baselines come from:

- project instruction files
- global and project `Memory`
- successful full-file `read_file` snapshots

Partial reads do not create or refresh a tracked baseline. Tool-based writes refresh an existing tracked baseline for that path.

## Mental model

`Memory` is shared background context, not the active transcript. Session history lives in `events.jsonl`; `Memory` lives in markdown files and is loaded as prelude context.

For the maintainer view of how `Memory` enters a request, see [Prompt Assembly](../architecture/prompt-assembly.md).
