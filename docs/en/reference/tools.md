# Tools

This page describes the default public Session tool surface used by CLI and server. Presets or explicit tool selection can narrow it.

## Built-In Tools

| Tool | Required args | Optional args | Risk |
|---|---|---|---|
| `find_files` | none | `path`, `pattern`, `max_depth` | `read` |
| `read_file` | `path` | `offset`, `limit` | `read` |
| `read_output` | `id` | `offset`, `limit`, `search` | `read` |
| `search_text` | `pattern` | `path`, `include`, `view`, `offset`, `limit`, `context` | `read` |
| `fetch_url` | `url` | none | `read` |
| `query_session` | none | `search`, `kind`, `conversation`, `after`, `from_turn`, `to_turn`, `limit`, `skip_rolled_back` | `read` |
| `edit_file` | `path`, `old_text`, `new_text` | `replace_all`, `brief` | `edit` |
| `write_file` | `path`, `content` | `brief` | `edit` |
| `run_command` | `command`, `timeout` | `brief` | `command` |
| `remember_memory` | `scope`, `kind`, `text` | none | `edit` |
| `forget_memory` | `scope`, `text` | none | `edit` |

Tool guidance follows the actual tool selection. Reply-only Agents receive no tool workflow. Collaboration and memory-writing instructions are included only for Agents with the corresponding capabilities. The optional `brief` on file edits, writes and commands is a display label.

Conditional skill tools:

- `list_skills(offset?, limit?)`
- `search_skills(query, offset?, limit?)`
- `use_skill(skill_name)`

When the default skill tool surface is enabled, the runtime exposes `list_skills`, `search_skills`, and `use_skill` together.

## Session Coordination Tools

| Tool | Required args | Optional args | Risk |
|---|---|---|---|
| `ask_user` | `question` | none | `session` |
| `create_agent` | `address` | `instructions`, `tools`, `preset`, `tier`, `workspace` | `session` |
| `send_message` | `to`, `message` | `wake` | `session` |
| `list_agents` | none | `wait_for` | `session` |

`create_agent` creates a stable, flat Session-local address with independent context; an optional `preset` supplies the contact card. Supply the instructions and task material the recipient needs. `send_message` addresses an existing Agent and returns a durable delivery receipt, not its final work result. Its `wake` defaults to true; false queues the input for explicit advancement. `list_agents` returns a compact status list. Pass `wait_for` with one Agent address to wait up to 30 seconds for its current execution and retrieve its settled reply. A timeout returns the current state.

Model-facing creation takes the address, role instructions, tool allowlist, preset, tier and working directory. The host owns explicit provider settings through its SDK configuration.

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

## Retained Tool Output

`read_output` reads a Session-owned output ID returned by a tool such as `run_command`. It uses the same bounded reader as `Session::read_output`; it accepts no filesystem path. The tool remains subject to host authorization even though it is read-only.

| Argument | Contract |
|---|---|
| `id` | Required output ID: 1–128 ASCII letters, digits, `-`, or `_` |
| `offset` | Nonnegative raw byte offset across stdout and stderr in capture order; defaults to `0` |
| `limit` | Maximum raw bytes to scan: `1`–`8192`; defaults to `4096` |
| `search` | Optional literal filter: `1`–`4096` UTF-8 bytes, no longer than `limit` |

The result is complete JSON in `model_content`, at most 16,384 UTF-8 bytes; `structured` contains the same value. Encoding overhead or replacement of invalid UTF-8 can reduce the returned page below the requested scan limit. The tool keeps the reader's raw byte offsets and never cuts the JSON text. A search that cannot fit the result bound even at its minimum scan window returns an error asking for a shorter literal.

`segments` preserve stream identity, capture order, raw offsets and byte lengths. `next_offset` and `has_more` control continuation; always pass `next_offset` to the next read instead of adding `limit`. Literal search can retain overlap between scan windows. An empty matching page with `has_more: true` still requires continuation. `filtered` identifies search results and `lossy_utf8` reports replacement of invalid or split UTF-8 bytes.

When a segment's `text` replaces invalid or split UTF-8, `raw_bytes` carries its original bytes. Otherwise its UTF-8 text represents those bytes directly. SDK callers can use `OutputSegment::as_bytes()` for either case.

`complete` reports writer finalization. `capture_truncated` reports discarded output at the capture quota; `cleanup_incomplete` reports a pipe cleanup deadline. These flags remain explicit even when the returned JSON page fits its limit. `summary` contains the reader's selected build/test lines. See [Output and cleanup](../how-it-works/sessions.md#output-and-cleanup) for capture lifetime and cleanup semantics.

Offsets are supplied by each caller. Repeated reads of finalized output do not consume it, and UI reads cannot advance a model cursor. Invalid IDs, argument types, scan limits, or search lengths produce an error without an out-of-range read.

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
- `run_command` requires `timeout` in seconds. On Windows its background shell runs without opening a console window. Standard output and error remain available in the tool result.
- `remember_memory` and `forget_memory` write memory files through dedicated APIs.

## Memory Tool Enums

For `remember_memory`:

- `scope`: `global` or `project`
- `kind`: `how_to_work`, `what_is_true`, or `where_to_look`

For `forget_memory`:

- `scope`: `global` or `project`
