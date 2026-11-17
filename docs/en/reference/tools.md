# Tools

This page describes the default public Session tool surface used by CLI and server. Presets or explicit tool selection can narrow it.

## Built-In Tools

| Tool | Required args | Optional args | Risk |
|---|---|---|---|
| `find_files` | none | `path`, `pattern`, `max_depth` | `read` |
| `read_file` | `path` | `offset`, `limit` | `read` |
| `search_text` | `pattern` | `path`, `include`, `view`, `offset`, `limit`, `context` | `read` |
| `fetch_url` | `url` | none | `read` |
| `query_session` | none | `search`, `kind`, `conversation`, `after`, `from_turn`, `to_turn`, `limit`, `skip_rolled_back` | `read` |
| `edit_file` | `path`, `old_text`, `new_text`, `brief` | `replace_all` | `edit` |
| `write_file` | `path`, `content`, `brief` | none | `edit` |
| `run_command` | `command`, `timeout`, `brief` | none | `command` |
| `remember_memory` | `scope`, `kind`, `text` | none | `edit` |
| `forget_memory` | `scope`, `text` | none | `edit` |

Conditional skill tools:

- `list_skills(offset?, limit?)`
- `search_skills(query, offset?, limit?)`
- `use_skill(skill_name)`

When the default skill tool surface is enabled, the runtime exposes `list_skills`, `search_skills`, and `use_skill` together.

## Session Coordination Tools

| Tool | Required args | Optional args | Risk |
|---|---|---|---|
| `ask_user` | `question` | none | `session` |
| `create_agent` | `address` | `connection`, `tier`, `model`, `preset`, `instructions` | `session` |
| `send_message` | `to`, `message` | `wake` | `session` |
| `list_agents` | none | none | `session` |

`create_agent` creates a stable, flat Session-local address with independent context; an optional `preset` supplies the contact card. Supply the instructions and task material the recipient needs. `send_message` addresses an existing Agent and returns a durable delivery receipt, not its final work result. Its `wake` defaults to true; false queues the input for explicit advancement. `list_agents` reads retained states and latest durable outputs without consuming observations.

`ask_user` persists a question and yields at the current tool-batch boundary. The host replies to its exact request identity through the SDK, CLI `reply`, or server response endpoint. Question and authorization waiting semantics are defined in [Sessions](../how-it-works/sessions.md#waiting-and-replies).

`--no-agents` (or `AgentConfig.no_agents`) removes `create_agent`, `send_message`, and `list_agents` while retaining `ask_user`. See [CLI](cli.md#kuku-run) for host flags.

## Conversation and Ledger Inspection

`query_session` reads historical session events that are no longer in the visible conversation context.

Important filters:

- `conversation`: limit to one conversation address
- `kind`: limit to one event kind
- `after`: only events with id greater than this value
- `from_turn` and `to_turn`: relative turn window
- `skip_rolled_back`: defaults to `true`

`query_session` is for historical recall, not for data already present in the current messages.

## Tool Result Envelope

Every tool returns the same top-level shape:

| Field | Meaning |
|---|---|
| `status` | `ok`, `error`, `blocked`, or `cancelled` |
| `summary` | Short outcome line |
| `model_content` | Evidence for the next step |
| `truncated` | Whether `model_content` was cut |
| `structured` | Optional machine-readable detail |

Snapshot-related `edit_file` and `write_file` errors retain `structured.kind: "error"` and add one recovery-oriented `reason_code`: `snapshot_required`, `full_snapshot_required`, `snapshot_stale`, or `old_text_not_visible`.

Within one advancing Agent, Kuku serializes `read_file`, `edit_file`, `write_file`, `remember_memory`, `forget_memory`, and `run_command` slots against each other. Command write paths cannot be determined statically, so independent commands may also wait; other read-only tools remain concurrent. Different Agents can advance concurrently in the same Session. This ordering covers only Kuku-managed slots, not external processes, so snapshot hash checks still apply.

## Notes By Tool

- `find_files` returns relative paths and skips common build directories.
- `read_file` returns line-numbered content and supports pagination.
- `search_text` is regex-based and supports `files`, `lines`, and `count` views.
- `fetch_url` downloads to a temp directory, rejects non-HTTP(S) URLs and embedded credentials, and enforces a 50 MB limit.
- `query_session` filters the session ledger, defaults to excluding rolled-back events, and truncates individual event content.
- `edit_file` requires a unique `old_text` match inside a prior read snapshot that is still visible in the active conversation's effective history. A partial or truncated read authorizes only the raw file text actually shown, while `replace_all=true` requires a full-file snapshot. LF input is mapped to CRLF only when needed to match a CRLF file, and the replacement preserves that line-ending style. The file hash must still match at write time.
- `write_file` overwrites only after a prior full-file read snapshot that is still visible in the active conversation's effective history.
- `run_command` requires `timeout` in seconds.
- `remember_memory` and `forget_memory` write memory files through dedicated APIs.

## Memory Tool Enums

For `remember_memory`:

- `scope`: `global` or `project`
- `kind`: `how_to_work`, `what_is_true`, or `where_to_look`

For `forget_memory`:

- `scope`: `global` or `project`
