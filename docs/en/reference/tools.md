# Tools

This page describes the default public Session tool surface used by CLI and server. Presets or explicit tool selection can narrow it.

## Built-In Tools

| Tool | Required args | Optional args | Risk |
|---|---|---|---|
| `find_files` | none | `path`, `pattern`, `max_depth` | `read` |
| `read_file` | `path` | `offset`, `limit` | `read` |
| `view_image` | `path` | none | `read` |
| `stat_file` | `path` | `version` | `read` |
| `read_output` | `id` or `name` | `offset`, `limit`, `search` | `read` |
| `search_text` | `pattern` | `path`, `include`, `view`, `offset`, `limit`, `context` | `read` |
| `fetch_url` | `url` | none | `read` |
| `query_session` | none | `search`, `kind`, `conversation`, `after`, `from_turn`, `to_turn`, `limit`, `skip_rolled_back` | `read` |
| `edit_file` | `path`, `old_text`, `new_text` | `replace_all`, `expected_version`, `brief` | `edit` |
| `write_file` | `path`, `content` | `expected_version`, `brief` | `edit` |
| `remove_file` | `path` | `expected_version`, `brief` | `edit` |
| `run_command` | `program` | `args`, `cwd`, `background`, `title` | `command` |
| `run_js` | `code` | `background`, `title` | `execute` |
| `list_jobs` | none | `before` | `session` |
| `wait_job` | `job_id` | none | `session` |
| `cancel_job` | `job_id` | none | `session` |
| `save_value` | `name` and either `output_id` or `value` | `replace` | `session` |
| `list_values` | none | `prefix`, `after`, `limit` | `read` |
| `remember_memory` | `scope`, `kind`, `text` | none | `edit` |
| `forget_memory` | `scope`, `text` | none | `edit` |

Tool guidance follows the actual tool selection. Reply-only Agents receive no tool workflow. Collaboration and memory-writing instructions are included only for Agents with the corresponding capabilities. The optional `brief` on file edits and writes is a display label.

`view_image` is offered by Session on the built-in DeepSeek Responses route when the model declares image input. It accepts a workspace PNG or JPEG up to 1 MiB and applies the same path authorization as file reads. Model requests include up to four distinct recent images that fit the remaining request byte budget, newest first. A tool result whose image is omitted still names the snapshot. [DeepSeek Responses image input](https://api-docs.deepseek.com/guides/responses_api/#image-input)

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

`create_agent` creates a stable, flat Session-local address with independent context; an optional `preset` supplies the contact card. Supply the instructions and task material the recipient needs. `send_message` addresses an existing Agent and returns a durable delivery receipt, not its final work result. Its `wake` defaults to true; false queues the input for explicit advancement. `list_agents` returns a compact status list. Pass `wait_for` with one Agent address to wait up to 30 seconds for its current execution and retrieve its settled reply. A timeout returns the current state. A settled reply can include `last_request` with the model and the requested and provider-reported service tiers. Fields without available evidence are omitted.

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

Pass the executable as `program` and one literal string per argument in `args`. Omit `args` for a program with no arguments. Empty strings, spaces, quotes, and shell metacharacters can be parts of arguments. The working directory defaults to the Agent's workspace. Use `cwd` for a different workspace-relative or absolute directory. Prefix an executable path relative to `cwd` with `./` on any platform. Standard input supplies EOF.

```json
{"program":"git","args":["status","--short"]}
```

If the task requires a shell script, supply its interpreter and arguments explicitly. For example, on Windows a batch file can run with `{"program":"cmd.exe","args":["/D","/C","build.cmd"]}`.

```json
{"program":"sh","args":["./verify.sh"]}
```

The [embedding host](sdk.md#workspace-execution) can set a process deadline. The same [Session Job](#session-jobs) behavior applies to commands and scripts. An optional top-level `title` explains the action's purpose on its card and while its Job is running.

An exited process can return tool status `ok` with a nonzero `exit_code`. A shell invocation reports the outer shell's exit code. Preserve each required check's failure when composing commands or pipelines. The command card shows elapsed time and flags stderr separately from the exit status. Check `termination`, `exit_code`, and `capture_complete` in the structured facts.

When the Agent has `read_output`, short command output appears inline. Output above 2 KiB or 40 lines returns a bounded view and retained output reference. The view gives recognized build/test evidence or describes each stream's observed prefix, including bounded samples for multiline text. For JSON Lines, the view can show observed keys and how to process the data through an available script. Use `read_output` to inspect a relevant range. The TUI can show an excerpt independently.

For recognized human output from a long native Cargo build, check, Clippy, test, or ordinary Pytest run, the view can summarize progress and successful test lines while showing diagnostics and failing assertions. A summary names its rule and gives an example byte range to reopen. The structured `output_summary` includes the stream and half-open merged offsets for its source ranges. The raw capture remains available through `read_output`. Explicit machine-readable modes, test discovery, and incomplete captures use the generic preview.

Long plain TypeScript compiler reports can group repeated errors by exact code and message. Each group includes its count and sample source locations. Pretty and multiline diagnostics use the generic preview.

Long plain `go test` reports can count package results while retaining test failures and compiler diagnostics with their source ranges. JSON, verbose and unrecognized reports use the generic preview.

Long `node --test` spec reports can summarize test counts and retain failing assertions, TODO diagnostics, and their source ranges. Other reporters and unrecognized reports use the generic preview.

Long Git short status and line-oriented porcelain v1 can group rows by staged, unstaged, conflicting, untracked and ignored state. The view keeps counts, representative rows with Git quoting, and a hint for filtering each raw status row in `run_js`. Other Git status formats use the generic preview. [Git status formats](https://git-scm.com/docs/git-status#_output)

The default Session preview is bounded to 8 KiB, including its summary and retained reference. A complete capture can still have a truncated preview. For a long, failed command with complete capture, the generic preview includes the end of stderr or stdout when stderr is empty. Recognized test counts and selected build or test lines can accompany the excerpt. Missing executables, start failures, timeouts, and cancellation have separate termination values. Recognized Rust test and pytest summaries carry program-reported counts and bounded evidence. [SDK execution facts](sdk.md#workspace-execution)

## Session Jobs

A top-level `run_command` or `run_js` returns its outcome when it settles within the observation window. The default foreground window is 10 seconds. Use `background: true` for a one-second window and continue independent work if the call returns a Job. If execution is still running at the boundary, the tool returns a handle with `structured.kind: "job"` and `structured.job_id`. The handle includes `on_session_close: "cancel"` in both the model result and the structured facts.

[Job lifetime and result delivery](../how-it-works/sessions.md#background-jobs) · [Permissions](../how-it-works/permissions.md)

- `list_jobs({})` returns up to 25 of the current Agent's Job summaries, newest first. Pass the returned `next_before` as `before` for the next page.
- `wait_job({job_id})` waits for one of the current Agent's Jobs to settle. Use it when the next action needs that result.
- `cancel_job({job_id})` requests cancellation of one of the current Agent's Jobs and waits for cleanup.

Use the command's retained output reference to inspect its capture. [Job API for embedding hosts](sdk.md#workspace-execution)

## JavaScript tool composition

`run_js({code})` runs an async JavaScript function body with top-level `await` in a disposable, embedded QuickJS worker. Use `background: true` on the outer tool call to start a [Session Job](#session-jobs). Use standard JavaScript, including `JSON`, regular expressions and `Promise.all` for independent operations. Compose dependent actions with `await` when their inputs follow directly from earlier results. Return a short observation when the next decision needs model judgment. The methods on `tools` reflect the Agent's selected tools. `tools.methods` lists the available host methods and input schemas.

When the compiler supplies a location for a syntax error, the line and column refer to the submitted `code`.

Most host methods return `{status, result, truncated}`. Check for `status: "ok"` before using the structured `result`. An unsuccessful action includes an `error` message. Check `truncated` before treating a listing or page as complete. For example, `tools.read_file({path})` exposes raw file text in `result.raw_text`, the SHA-256 `content_hash` and the half-open byte range from `start_byte` to `end_byte`. A script can use these observed ranges for a subsequent edit without returning file contents to the model. Complete read coverage of the same file version also supports overwrite and removal. A script can edit or overwrite the exact version of its own confirmed full write within the same invocation. The actions still pass through host authorization and version checks.

Return a small JSON value to give the model the evidence it needs. A script that finishes without a value can use `console.log(...values)` to return bounded text. An explicit value, including `null`, takes precedence. To process large captures, iterate `tools.output_lines({id, stream: "stdout"})` and keep only the state needed for the computation. The original bytes remain available through [retained output](#retained-tool-output). Each call owns its JavaScript variables until it finishes. Persist selected results as a [named value](#named-session-values) or write a requested deliverable to the workspace.

`await tools.run_command({program, args?, cwd?})` waits for the command to settle. To detach the script, set `background: true` on the outer `run_js` call. A process exit with an integer code resolves to `{exit_code, duration_ms, capture_complete, output_ref, stdout, stderr}`, including on nonzero exit. Actions such as a denied dispatch, a launch failure, a timeout or cancellation reject with an error. The error includes `status` and carries `termination`, `signal`, or `output_ref` when available.

For either stream, `await run.stdout.text()` reads the complete decoded text up to 1 MiB, and `await run.stdout.json()` parses that text as JSON. Use `for await (const line of run.stderr.lines())` to stream decoded text without LF or CRLF terminators. Lines can contain up to 128 KiB each. The helpers reject incomplete or lossy output. Use `tools.output_chunks({id: run.output_ref.id})` to inspect retained partial output or raw bytes. Reads that page through retained output use the authorized `read_output` method.

For example, check a Git working tree and return a count derived from the complete status output.

```javascript
const run = await tools.run_command({program: "git", args: ["status", "--porcelain"]});
const {exit_code, duration_ms, capture_complete, output_ref} = run;
if (exit_code !== 0 || !capture_complete) {
  return {exit_code, duration_ms, capture_complete, output_ref};
}
const entries = (await run.stdout.text()).split("\n").filter(Boolean).length;
return {exit_code, duration_ms, entries, output_ref};
```

`output_chunks({id, offset?, limit?, search?})` iterates the bounded output pages with original stream segments and raw bytes. The default scan window is 65,536 bytes, with at most 80,000 bytes per request. `output_lines({id, stream?, offset?, limit?})` iterates decoded lines without their trailing newline and takes unfiltered pages. Both accept `name` in place of `id` and keep reading the original output ID once the iterator resolves the name. Each line carries its stream, byte offsets and completeness flags. Script host methods report denied, failed and cancelled actions independently of the script's return value.

`await tools.search_output({id, text, stream?, offset?, limit?})` finds exact UTF-8 bytes in retained output. It accepts `name` in place of `id` and uses the same `read_output` authorization. Each request scans up to 8 MiB of new retained bytes in Rust and returns up to 32 matches by default, with a maximum of 128. Check `status` before using `result`, then continue with `result.output_id` and `result.next_offset` while `result.has_more` is true. This includes empty pages.

Matches are ordered by completion in the capture. Each match gives `stream`, `offset` and exclusive `end_offset` in the merged byte coordinates. A match can span frames of one stream, so filter by its stream when reading back a range interleaved with the other stream. `capture_complete` describes the original capture, separately from whether the scan has more pages.

For a large derived result, take `result.id` from `await tools.create_output({})`, then `await tools.append_output({id, text})` for each batch of UTF-8 text. Check each action's status. Call `await tools.finish_output({id})` and check `result.capture_complete` before relying on the new `result.output_ref`. The original capture remains available independently.

An output draft belongs to the current script invocation. See the [authorization rules for draft actions](../how-it-works/permissions.md).

## Retained Tool Output

`read_output` reads retained output by ID or the current Agent's named value by exact name. With a running command's output ID, it returns the complete capture frames available at the time of the read. Every call passes host authorization. The limits below apply to the direct tool. For the script reader's schema, see `tools.methods`.

| Argument | Contract |
|---|---|
| `id` | Output ID of 1–128 ASCII letters, digits, `-`, or `_`. Use exactly one of `id` and `name`. |
| `name` | Exact name of a saved value owned by the Agent |
| `offset` | Nonnegative raw byte offset across stdout and stderr in capture order; defaults to `0` |
| `limit` | Positive raw byte budget. Defaults to `4096` and is capped at `8192`. |
| `search` | Optional literal filter: `1`–`4096` UTF-8 bytes, no longer than `limit` |

The result is complete JSON in `model_content`, at most 16,384 UTF-8 bytes; `structured` contains the same value. Encoding overhead or replacement of invalid UTF-8 can reduce the returned page below the requested scan limit. The tool keeps the reader's raw byte offsets and never cuts the JSON text. A search that cannot fit the result bound even at its minimum scan window returns an error asking for a shorter literal.

`segments` preserve stream identity, capture order, raw offsets and byte lengths. `next_offset` and `has_more` control continuation; always pass `next_offset` to the next read instead of adding `limit`. Literal search can retain overlap between scan windows. An empty matching page with `has_more: true` still requires continuation. `filtered` identifies search results and `lossy_utf8` reports replacement of invalid or split UTF-8 bytes.

The text view removes ANSI color codes. On Windows, it reads UTF-8 first and tries the host OEM code page for intact text lines on either pipe. Binary data and partial characters can still require replacement. Whenever the display text differs from the captured bytes, `raw_bytes` carries the original bytes. Otherwise its UTF-8 text represents those bytes directly. SDK callers can use `OutputSegment::as_bytes()` for either case.

`complete` reports writer finalization. When both `has_more` and `complete` are false, the read has reached the currently available end of the capture. To check for later output, reuse `next_offset`. `capture_truncated` reports discarded output at the capture quota. `cleanup_incomplete` reports a pipe cleanup deadline. `summary` contains the reader's selected build/test lines. See [Output and cleanup](../how-it-works/sessions.md#output-and-cleanup) for capture lifetime and cleanup semantics.

Offsets are supplied by each caller. Repeated reads of finalized output do not consume it, and UI reads cannot advance a model cursor. Invalid IDs, argument types, scan limits, or search lengths produce an error without an out-of-range read.

## Named Session values

Use `save_value({name, output_id})` to give an existing capture a durable name without copying its bytes, or `save_value({name, value})` to store a literal JSON value of up to 16 KiB. Strings retain their UTF-8 text. Names are case-sensitive, start with an ASCII letter and contain up to 64 ASCII letters, digits, underscores or hyphens.

Use `list_values({prefix?, after?, limit?})` to discover names, kinds, and references. The result contains `values` and `next_after`. It returns at most 32 entries in name order. Pass `next_after` as `after` to continue. Read contents on demand with `read_output({name})`. Bindings belong to the current Agent and survive closing and reopening the Session. Saving the same source again is idempotent. Supply `replace: true` to bind an existing name to a new source.

When these tools are available to a script, call `await tools.save_value({name, value})` to retain a computed value or pass `output_id` to bind a retained output. Use `await tools.list_values()` to discover the current Agent's names. Both methods use the same arguments and authorization as their direct tools.

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

Within one advancing Agent, kuku serializes the `read_file`, `stat_file`, `edit_file`, `write_file`, `remove_file`, `remember_memory`, `forget_memory`, `run_command`, and `run_js` slots. Other read-only tools can overlap them. Different Agents can advance concurrently in the same Session. Snapshot hash checks protect file operations against changes between reads and writes, including changes by external processes.

## Notes By Tool

- `find_files` lists immediate entries by default and searches recursively when given a `pattern` glob. `max_depth` overrides the default, with `0` for immediate entries and `1` to search one directory level below. Relative directory paths end in `/`. The scan skips common build directories. Large listings report the total and suggest a narrower query.
- `read_file` returns line-numbered content and supports pagination.
- `stat_file` returns the path type (`file`, `directory`, `other`, or `missing`), existence and available size. With `version: true`, it streams the bytes of a regular file to compute its SHA-256 `content_hash` and returns the hash without the file contents.
- `search_text` is regex-based and supports `files`, `lines`, and `count` views. Structured `matches` give paths and line numbers. `lines` also gives bounded text. Use `next_offset` to continue. Use the matching text as edit context or read the surrounding lines for more context.
- `fetch_url` downloads an HTTP(S) response body up to 50 MiB. The result includes the HTTP status even for an error response. In a Session, the complete body must fit the available output quota. Once the download finishes, the requesting Agent can pass `output_ref.id` to `read_output`. Cancelling a download discards the partial body. A standalone query returns a temporary file path. URLs with embedded passwords are rejected.
- `query_session` filters the session ledger, defaults to excluding rolled-back events, and truncates individual event content.
- `edit_file` can replace known `old_text` directly when it occurs exactly once in the current file. Edits to other parts of the file are preserved. For repeated text, the tool accepts a single match within the observed ranges of the current version. Supply more surrounding text if several locations qualify. `replace_all=true` requires knowledge of the whole file version. LF input is mapped to CRLF only when needed to match a CRLF file, and the replacement preserves that line-ending style. The file hash must still match at write time.
- `write_file` creates any missing parent directories inside the workspace when it creates a file. It can overwrite a file version that the Agent has fully read or supplied through its own confirmed `write_file` in its effective history. The current path and hash must match that evidence. A partial edit does not establish knowledge of the resulting whole file.
- `remove_file` removes a regular file when `expected_version` matches the current hash or, if omitted, after a whole-file read of the same version. For a binary file, obtain the hash with `stat_file({path, version: true})` and pass it as `expected_version`. A final symlink is rejected.
- File tools accept the returned `sha256:` version or the same full 64-digit SHA-256 digest as `expected_version`. The content check uses all digest bits.
- `edit_file` and `write_file` accept `expected_version` with the `content_hash` of an earlier read or confirmed write as an additional version check. Use this when the operation must fail on any change to the whole file since that observation.
- `remember_memory` and `forget_memory` write memory files through dedicated APIs.

## Memory Tool Enums

For `remember_memory`:

- `scope`: `global` or `project`
- `kind`: `how_to_work`, `what_is_true`, or `where_to_look`

For `forget_memory`:

- `scope`: `global` or `project`
