# Glossary

## Core Runtime Terms

| Term | Meaning |
|---|---|
| `Workspace` | The project root directory used for one run |
| `kuku home` | Runtime home directory, default `~/.kuku` |
| `Session` | One persisted execution history stored as a directory |
| `Agent` | Stable address, independent context and pending work inside one Session |
| `Turn` | A round of work admitted by an Agent, possibly spanning several model requests |
| `Step` | One logical model request and the tool calls its response produces |
| `events.jsonl` | Append-only session fact log |
| `Memory` | Long-lived context stored in `memory.md` files |

## Model and Context Terms

| Term | Meaning |
|---|---|
| `provider` | The model API adapter |
| `tier` | A named model preset such as `strong`, `balanced`, or `light` |
| `think level` | Provider reasoning level: `off`, `low`, `medium`, or `high` |
| `handoff` | A summary written when long history is compressed |

## Extension Terms

| Term | Meaning |
|---|---|
| Agent preset | A discoverable definition supplying the initial instructions, tier and tools for an Agent |
| `Skill` | A packaged instruction set loaded into the current session |
| `package` | A bundle of hooks, skills, and related assets |
| `hook` | An external process triggered by runtime lifecycle events |

## Tooling Terms

| Term | Meaning |
|---|---|
| `tool` | A callable capability with a schema and risk level |
| `risk` | Tool risk class, such as `read`, `edit`, `command` or `session` |
| `permission` | A runtime authorization decision before tool execution |
| `wire event` | Client-facing NDJSON event emitted by the server |
