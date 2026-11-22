# Glossary

## Core runtime

| Term | Meaning |
|---|---|
| `Workspace` | Project root directory used for a run |
| `kuku home` | Runtime data directory (default `~/.kuku`) |
| `Session` | Persisted execution history stored in one directory |
| `Agent` | Named participant inside a Session with independent context and inbox |
| `Turn` | One round of work admitted by an Agent (may span multiple model requests) |
| `Step` | One logical model request and the tool calls its response triggers |
| `events.jsonl` | Append-only Session fact journal |
| `Memory` | Durable cross-Session notes stored in `memory.md` files |

## Models and context

| Term | Meaning |
|---|---|
| `Provider` | Model API adapter and connection configuration |
| `Tier` | Named model preset such as `strong`, `balanced`, or `light` |
| `Think level` | Portable reasoning level: `off`, `low`, `medium`, or `high` |
| `Handoff` | Compaction summary written to open a successor context segment |

## Extensions

| Term | Meaning |
|---|---|
| Agent preset | Discoverable Markdown file defining initial instructions, tier, and tools |
| `Skill` | Packaged instruction set loaded into the current Session |
| `Package` | Bundle of hooks, Skills, and related assets |
| `Hook` | External process triggered by runtime lifecycle events |

## Tooling and wire protocol

| Term | Meaning |
|---|---|
| `Tool` | Callable capability with a JSON schema and risk class |
| `Risk` | Tool risk class (`read`, `edit`, `command`, `session`, etc.) |
| `Permission` | Runtime authorization decision before tool execution |
| `Wire event` | Client-facing NDJSON event emitted by the HTTP server |
