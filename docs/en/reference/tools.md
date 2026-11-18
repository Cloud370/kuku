# Tools

This page describes the default public Session tool surface used by CLI and server. Presets or explicit tool selection can narrow it.

## Built-In Tools

| Tool | Required args | Optional args | Risk |
|---|---|---|---|
| `find_files` | none | `path`, `pattern`, `max_depth` | `read` |
| `read_file` | `path` | `offset`, `limit` | `read` |
| `stat_file` | `path` | `version` | `read` |
| `read_output` | `id` or `name` | `offset`, `limit`, `search` | `read` |
| `search_text` | `pattern` | `path`, `include`, `view`, `offset`, `limit`, `context` | `read` |
| `fetch_url` | `url` | none | `read` |
| `query_session` | none | `search`, `kind`, `conversation`, `after`, `from_turn`, `to_turn`, `limit`, `skip_rolled_back` | `read` |
| `edit_file` | `path`, `old_text`, `new_text` | `replace_all`, `expected_version`, `brief` | `edit` |
| `write_file` | `path`, `content` | `expected_version`, `brief` | `edit` |
| `remove_file` | `path` | `expected_version`, `brief` | `edit` |
| `run_command` | `cwd`, `timeout`, and either `program` + `args` or `shell` + `command` | `brief` | `command` |
| `workspace_script` | `code` | none | `execute` |
| `save_value` | `name` and either `output_id` or `value` | `replace` | `session` |
| `list_values` | none | `prefix`, `after`, `limit` | `read` |
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
| `send_message` | `to`, `message` | `wake`, `values` | `session` |
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

## Run a native program

Use the program name and one literal string per argument. Prefix an executable path relative to `cwd` with `./` on any platform. Pass `args: []` for a program with no arguments. Empty strings, spaces, quotes, and shell metacharacters can be parts of arguments. `cwd` can be a workspace-relative or absolute directory. Use `"."` for the workspace root. `timeout` is an integer between 1 and 600 seconds. The program receives EOF on standard input.

```json
{"program":"cargo","args":["test","-p","kuku"],"cwd":".","timeout":120}
```

For pipes, redirection, or shell built-ins, choose an interpreter explicitly. The tool schema advertises the interpreters found on the execution host from `sh`, `bash`, `zsh`, `cmd`, `powershell` (Windows PowerShell), and `pwsh` (PowerShell Core). On Windows, use `shell: "cmd"` for batch files or commands that use `cmd.exe`.

```json
{"shell":"cmd","command":"type \"build log.txt\"","cwd":".","timeout":10}
```

An exited process can return tool status `ok` with a nonzero `exit_code`. Check `termination`, `exit_code`, and `capture_complete` in the structured facts. Missing executables, start failures, timeouts, and cancellation have separate termination values. Recognized Rust test and pytest summaries carry program-reported counts and bounded evidence. Use the retained output to investigate failures or formats without a recognized report. See [SDK execution facts](sdk.md#workspace-execution).

## Workspace scripts

`workspace_script({code})` runs an async JavaScript function body with top-level `await` in a disposable, embedded QuickJS worker. The worker ships with kuku and starts on demand. Use standard JavaScript, including `JSON`, regular expressions and `Promise.all` for independent operations. Access the workspace through the `workspace` object. Its methods depend on the Agent's selected tools and take the same object arguments as their direct counterparts. `workspace.methods` lists the available definitions.

Each host call returns `{status, summary, truncated, result, model_content}`. `result` contains the structured data. For example, `workspace.read_file({path})` exposes raw file text in `result.raw_text`, the SHA-256 `content_hash` and the half-open byte range from `start_byte` to `end_byte`. A script can use these observed ranges for a subsequent edit without returning file contents to the model. Complete coverage of the same file version also supports overwrite and removal. The actions still pass through host authorization and version checks.

Return a small JSON value to give the model the evidence it needs. To process large captures, iterate `workspace.output_lines({id, stream: "stdout"})` and keep only the state needed for the computation. The original bytes remain available through [retained output](#retained-tool-output). Each call owns its JavaScript variables until it finishes. Persist selected results as a [named value](#named-session-values) or write a requested deliverable to the workspace.

For example, pass the following JavaScript as `code` to count warnings on stderr in a Rust build. A failed build still yields its observed exit code and capture reference when available.

```javascript
const run = await workspace.run_command({
  program: "cargo", args: ["check"], cwd: ".", timeout: 20,
});
const facts = run.result;
if (!facts?.output_ref) return { action: run.status, summary: run.summary };

let warnings = 0;
let complete = facts.capture_complete;
for await (const line of workspace.output_lines({ id: facts.output_ref.id, stream: "stderr" })) {
  if (line.text.startsWith("warning:")) warnings++;
  complete &&= !line.truncated;
}
return { exitCode: facts.exit_code, warnings, complete, outputId: facts.output_ref.id };
```

`output_chunks({id, offset?, limit?, search?})` iterates the bounded output pages with original stream segments and raw bytes. The default scan window is 65,536 bytes, with at most 80,000 bytes per request. `output_lines({id, stream?, offset?, limit?})` iterates decoded lines without their trailing newline and takes unfiltered pages. Both accept `name` in place of `id` and keep reading the original output ID once the iterator resolves the name. Each line carries its stream, byte offsets and completeness flags. Script host methods report denied, failed and cancelled actions independently of the script's return value.

For a large derived result, take `result.id` from `await workspace.create_output({})`, then `await workspace.append_output({id, text})` for each batch of UTF-8 text. Check each action's status. Call `await workspace.finish_output({id})` and check `result.capture_complete` before relying on the new `result.output_ref`. The original capture remains available independently.

An output draft belongs to the current script invocation. Creating, appending to, and finishing a draft require an immediate authorization decision. A deferred decision yields a `blocked` result for that action.

## Retained Tool Output

`read_output` reads a retained output by ID or the current Agent's named value by exact name. It accepts Session references and uses the SDK's bounded reader. Every call passes host authorization. The limits below apply to the direct tool. For the script reader's schema, see `workspace.methods`.

| Argument | Contract |
|---|---|
| `id` | Output ID of 1–128 ASCII letters, digits, `-`, or `_`. Use exactly one of `id` and `name`. |
| `name` | Exact name of a saved value owned by the Agent |
| `offset` | Nonnegative raw byte offset across stdout and stderr in capture order; defaults to `0` |
| `limit` | Maximum raw bytes to scan: `1`–`8192`; defaults to `4096` |
| `search` | Optional literal filter: `1`–`4096` UTF-8 bytes, no longer than `limit` |

The result is complete JSON in `model_content`, at most 16,384 UTF-8 bytes; `structured` contains the same value. Encoding overhead or replacement of invalid UTF-8 can reduce the returned page below the requested scan limit. The tool keeps the reader's raw byte offsets and never cuts the JSON text. A search that cannot fit the result bound even at its minimum scan window returns an error asking for a shorter literal.

`segments` preserve stream identity, capture order, raw offsets and byte lengths. `next_offset` and `has_more` control continuation; always pass `next_offset` to the next read instead of adding `limit`. Literal search can retain overlap between scan windows. An empty matching page with `has_more: true` still requires continuation. `filtered` identifies search results and `lossy_utf8` reports replacement of invalid or split UTF-8 bytes.

The text view removes ANSI color codes. On Windows, it reads UTF-8 first and tries the host OEM code page for intact text lines on either pipe. Binary data and partial characters can still require replacement. Whenever the display text differs from the captured bytes, `raw_bytes` carries the original bytes. Otherwise its UTF-8 text represents those bytes directly. SDK callers can use `OutputSegment::as_bytes()` for either case.

`complete` reports writer finalization. `capture_truncated` reports discarded output at the capture quota; `cleanup_incomplete` reports a pipe cleanup deadline. These flags remain explicit even when the returned JSON page fits its limit. `summary` contains the reader's selected build/test lines. See [Output and cleanup](../how-it-works/sessions.md#output-and-cleanup) for capture lifetime and cleanup semantics.

Offsets are supplied by each caller. Repeated reads of finalized output do not consume it, and UI reads cannot advance a model cursor. Invalid IDs, argument types, scan limits, or search lengths produce an error without an out-of-range read.

## Named Session values

Use `save_value({name, output_id})` to give an existing capture a durable name without copying its bytes, or `save_value({name, value})` to store a literal JSON value of up to 16 KiB. Strings retain their UTF-8 text. Names are case-sensitive, start with an ASCII letter and contain up to 64 ASCII letters, digits, underscores or hyphens.

Use `list_values({prefix?, after?, limit?})` to discover names, kinds, and references. The result contains `values` and `next_after`. It returns at most 32 entries in name order. Pass `next_after` as `after` to continue. Read contents on demand with `read_output({name})`. Bindings belong to the current Agent and survive closing and reopening the Session. Saving the same source again is idempotent. Supply `replace: true` to bind an existing name to a new source.

When these tools are available to a script, call `await workspace.save_value({name, value})` to retain a computed value or pass `output_id` to bind a retained output. Use `await workspace.list_values()` to discover the current Agent's names. Both methods use the same arguments and authorization as their direct tools.

Use `send_message({to, message, values: ["build_log"]})` to give the recipient a fixed reference to the value along with its task. You can share up to eight exact names or whole-output references in one message. A later replacement of `build_log` leaves the delivered reference intact. The recipient can read that reference by its shared ID.

## Tool Result Envelope

Every tool returns the same top-level shape:

| Field | Meaning |
|---|---|
| `status` | `ok`, `error`, `blocked`, or `cancelled` |
| `summary` | Short outcome line |
| `model_content` | Evidence for the next step |
| `truncated` | Whether the evidence is truncated, including incomplete capture on a command |
| `structured` | Optional machine-readable detail |

Snapshot-related errors from file changes retain `structured.kind: "error"` and add a recovery-oriented `reason_code` such as `snapshot_required`, `full_snapshot_required`, `snapshot_stale`, or `old_text_not_visible`.

Within one advancing Agent, kuku serializes the `read_file`, `stat_file`, `edit_file`, `write_file`, `remove_file`, `remember_memory`, `forget_memory`, `run_command`, and `workspace_script` slots. Other read-only tools can overlap them. Different Agents can advance concurrently in the same Session. Snapshot hash checks protect file operations against changes between reads and writes, including changes by external processes.

## Notes By Tool

- `find_files` returns a bounded `entries` list of relative paths. Directories in the list end in `/`. The scan skips common build directories.
- `read_file` returns line-numbered content and supports pagination.
- `stat_file` returns the path type (`file`, `directory`, `other`, or `missing`), existence and available size. With `version: true`, it streams the bytes of a regular file to compute its SHA-256 `content_hash` and returns the hash without the file contents.
- `search_text` is regex-based and supports `files`, `lines`, and `count` views. Structured `matches` give paths and line numbers. `lines` also gives bounded text. Use `next_offset` to continue. Read the file itself before editing.
- `fetch_url` downloads to a temp directory, rejects non-HTTP(S) URLs and embedded credentials, and enforces a 50 MB limit.
- `query_session` filters the session ledger, defaults to excluding rolled-back events, and truncates individual event content.
- `edit_file` requires a unique `old_text` match inside a prior read snapshot that is still visible in the active conversation's effective history. A partial or truncated read authorizes only the raw file text actually shown, while `replace_all=true` requires a full-file snapshot. LF input is mapped to CRLF only when needed to match a CRLF file, and the replacement preserves that line-ending style. The file hash must still match at write time.
- `write_file` overwrites only after a prior full-file read snapshot that is still visible in the active conversation's effective history.
- `remove_file` removes a regular file when `expected_version` matches the current hash or, if omitted, after a whole-file read of the same version. For a binary file, obtain the hash with `stat_file({path, version: true})` and pass it as `expected_version`. A final symlink is rejected.
- File tools accept the returned `sha256:` version or the same full 64-digit SHA-256 digest as `expected_version`. The content check uses all digest bits.
- `edit_file` and `write_file` accept `expected_version` with the `content_hash` of an earlier read as an additional version check. Their snapshot requirements still apply.
- `remember_memory` and `forget_memory` write memory files through dedicated APIs.

## Memory Tool Enums

For `remember_memory`:

- `scope`: `global` or `project`
- `kind`: `how_to_work`, `what_is_true`, or `where_to_look`

For `forget_memory`:

- `scope`: `global` or `project`
