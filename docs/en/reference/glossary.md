# Glossary

## Core Runtime Terms

| Term | Meaning |
|---|---|
| `Workspace` | The project root directory used for one run |
| `kuku home` | Runtime home directory, default `~/.kuku` |
| `Session` | One persisted execution history stored as a directory |
| `turn` | One user-input to model-response cycle |
| `events.jsonl` | Canonical append-only event log for a session |
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
| `Agent` | A named subagent that runs in a child session |
| `Skill` | A packaged instruction set loaded into the current session |
| `package` | A bundle of hooks, skills, and related assets |
| `hook` | An external process triggered by runtime lifecycle events |

## Tooling Terms

| Term | Meaning |
|---|---|
| `tool` | A callable capability with a schema and risk level |
| `risk` | Tool risk class: `read`, `edit`, or `command` |
| `permission` | A runtime authorization decision before tool execution |
| `wire event` | Client-facing NDJSON event emitted by the server |
