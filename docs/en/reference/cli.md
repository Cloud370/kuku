# CLI

## Entry Modes

- `kuku` opens the full-screen client in an interactive terminal.
- `kuku run ...` starts a non-interactive run. Use it in scripts or when no interactive terminal is available.

The terminal, script and browser clients submit work to the server. The local clients reuse the managed service for the selected data directory. [Run the server](../guides/run-the-server.md)

First-run interactive setup and `kuku init` use the same local provider-settings flow. `kuku init` requires a TTY, can edit an existing valid config, and never starts a service to configure it. Remote settings are not supported. See [Configuration](../start/configuration.md).

## Connection and workspace flags

| Flag | Meaning |
|---|---|
| `-w`, `--workspace <path>` | Workspace for commands and file tools. Local clients default to the resolved current directory. For a remote Session, provide the absolute path on the server machine. |
| `--data-dir <path>` | Local service data directory. Defaults to `$KUKU_HOME` or `~/.kuku`. With `--remote`, it selects local client storage. |
| `--config <path>` | Select a specific local configuration file. Manage remote configuration on the server machine. |
| `--remote <url>` | Connect to this server explicitly. |
| `--token <token>` | Bearer token for the explicit server, also accepted via `KUKU_TOKEN`. |

Put options before the prompt in `kuku run`, because it consumes the remaining words as prompt text.

## Top-Level Commands

| Command | Purpose |
|---|---|
| `kuku run <prompt...>` | Submit a task and observe its execution |
| `kuku reply <session-id> <request-id>` | Answer a durable question or authorization |
| `kuku show <session-id>` | Show final output from a session |
| `kuku events <session-id>` | Show persisted session facts |
| `kuku list` | List sessions |
| `kuku pending ...` | Reconcile submissions whose receipt was lost |
| `kuku delete <session-id>` | Delete a session |
| `kuku config ...` | Show, validate, or change config |
| `kuku init` | Open shared local provider and credential settings |
| `kuku prompts ...` | Show or export prompt assets |
| `kuku agents ...` | List or inspect agents |
| `kuku skills ...` | List or inspect skills |
| `kuku server ...` | Start an explicit HTTP server |
| `kuku server status` | Inspect service identity and activity |
| `kuku server restart` | Restart the managed local service after outstanding work settles |
| `kuku web ...` | Open the browser interface on the selected service |

## Terminal interface

| Key or action | Behavior |
|---|---|
| Ctrl+C | Clear the draft. With an empty draft, exit the TUI. Running work continues on the server. Inside a dialog, clear the focused input. |
| Enter | Send the draft. A busy Agent accepts it at the next safe model-request boundary, or in the next Turn if the current Turn settles first. |
| Alt+Enter | Queue the draft for the next Turn. |
| Shift+Enter or Ctrl+J | Insert a newline in the draft. |
| Esc | Close the focused menu, or interrupt the selected execution and keep the draft. |
| Ctrl+P, F1, or `/` in an empty composer | Search available actions and Skills. Choosing a Skill inserts an editable `/skill:NAME` prefix for the next send. |
| `@` or Tab | Complete workspace-relative paths as draft text. |
| `/new`, `/resume` | Open a blank draft or find a retained Session. |
| Alt+← or Alt+→ | Switch directly to the previous or next Agent. The order wraps and starts with `main`, followed by the other addresses in sorted order. |
| `/agents` | Search the Session's Agents, switch recipients, or create an Agent. The selector shows activity, pending requests and queue counts, with `Admitted` and `Queued` task excerpts when available. |
| `/back` | Return to the last Agent you viewed in this Session. Repeat to switch between the last two. Agent switching keeps each draft and reading position. |
| `/queue`, `/withdraw`, `/continue` | Queue for the next Turn, withdraw a queued input, or explicitly resume work. |
| `/queued` | Edit a queued message before admission. With an empty draft, ↑ also opens this picker when an input can be edited. |
| F2 or `/reply` | Answer the selected Agent's pending question or authorization. |
| `/pending` | Check, retry or discard an unconfirmed submission. |
| `/model`, `/thinking` | Select model or reasoning settings for the selected Agent. |
| `/statusline` | Open **Status line settings** to preview, select, reorder and save footer fields. |
| Ctrl+O | Expand or collapse visible transcript previews, including tool groups and Agent cards. |
| `/details` or click a transcript item | Open a tool group to see its calls. Open an individual call for the target, arguments and available output or error. In **Transcript details**, search visible items and press Enter to toggle the selected item. |
| `/source` | Switch between the rendered view and original Markdown for assistant replies in the current Agent's transcript. The source view keeps the fence markers. |
| `/tools`, `/copy` | Open retained tool output or copy the complete source of the latest committed assistant reply. |
| Left-button drag | Highlight visible text and copy the selected range to the clipboard on release. |
| Mouse wheel, Page Up, Page Down | Scroll the transcript. |
| Ctrl+End or Alt+↑ while reading history | Return to the latest output and resume following. |
| ↑, ↓ in the editor | Move within a multiline draft and scroll the transcript at its boundaries. Ctrl+↑ and Ctrl+↓ recall sent prompts and restore the draft. |
| `/server`, `/reconnect`, `/restart` | Inspect the connected service, reconnect, or restart the managed local service. |
| Arrow keys, Tab, Enter in a menu | Move the highlight, complete editable text, or activate the selected action, respectively. |
| `/web`, `/quit` | Open the conversation in the browser or leave the terminal client. |

In a selection dialog, click a row to highlight it. A click can switch the viewed Agent or Session, open a model ID editor, or show transcript details and retained output. Applying a tier or reasoning setting, deciding an authorization, withdrawing an input or restarting still requires Enter.

### Reading the transcript

Each Turn contains user messages, tool activity and assistant text. Consecutive successful read-only inspections within a Turn are grouped by default, with counts for reads, searches and listings. File changes, running calls, failures and authorization requests stay individually visible. **List Agents** summarizes participants and their activity. **Query Session** summarizes matching events. Open details to inspect individual calls or read original results with `/tools`.

While reading earlier output, click **Jump to latest** beside the composer to resume following.

Completed Turn statistics appear once beneath the reply and scroll with it. When reading history, a floating Turn strip shares the composer’s left edge and text inset. It offers a return control without moving the input. It shows elapsed time, tokens, cache hits and tools as space permits.

The title of a command card is the short task description, or **Run command** if none was supplied. The card body previews the shell command and its output in execution order. The bottom border shows the status and exit code when available. Open the card for recorded arguments and capture details. Completed writes and edits share the code block frame. A write previews recorded file content with syntax highlighting or rendered Markdown. An edit shows the recorded diff. The footer shows the written or changed line counts.

Sent text appears immediately in **Pending inputs**, above the composer. The same input shows its send, receipt, queue or failure state until it is admitted to the transcript. Unconfirmed sends and queued inputs can be recovered on reopen. Edit a queued input with `/queued` or withdraw it with `/withdraw`. If the Agent admits the input first, its admitted text stays in the transcript.

Messages delivered by another Agent show the sender's address, for example `From main`, including when they open a Turn.

Markdown renders as it streams, including a code fence that is still open. A terminal that joins an active reply can show a bounded prefix of its already streamed text. The committed reply replaces that preview.

Markdown replies support task lists, nested quotes and lists, and inline `<sup>`, `<sub>` and `<br>`. Wrapped list text aligns with its item. Select an underlined HTTP or HTTPS link to open its destination in the default browser. Dragging over a link selects text. Code and the original source view keep their literal contents.

If you interrupt a streaming reply, its bounded preview stays under that Turn and remains visible after reopening the Session.

By default, a fence labeled `md` or `markdown` in an assistant reply previews the enclosed document, including its headings, lists, tables and code blocks. The label is case-insensitive. Inner code uses its declared language. Ordinary code fences display their contents literally, with syntax highlighting for supported languages.

Message, command, Agent, and file preview cards leave space around their content. Code blocks follow the same spacing. Side padding contracts with the terminal width. Agent cards and code blocks fill the available width at their level of indentation. A separate Agent card has its own border and a blank line between it and adjacent commands. Its Details control stays above the expanded task and reply. The Agent name serves as the title. Select **Open agent →** on a confirmed Agent card to switch to that Agent. Narrow cards shorten the action to **Open →**. Creation instructions appear under their own label and wrap across up to three preview lines. Open Details to read longer instructions. Task and reply excerpts each use a single preview line. A delegation card groups creation and delivery to the same Agent. Expand it to inspect the individual actions. Status checks involving only the sender and recipient join that card’s timeline. Checks involving other Agents keep their own counted group. A completed reply keeps its thinking reader beside the Assistant heading.

Thinking appears at the start of the assistant’s answer. The transcript types a muted, single-line excerpt when the provider publishes reasoning. The first line of the reply takes its place. Select **View thinking** or use `/notes` to read the received text at its position in the conversation. The reader shows up to five wrapped lines beside the reply. Use the arrow keys or the wheel over those lines to read more. Esc or a second click on the heading closes it. Copy and Markdown source use the reply text. Thinking from intermediate tool steps remains available through `/details`.

### Status and usage

An empty Session opens with the kuku title. On a spacious terminal, the session label and first actions align beneath the name. Once work starts, the Session header contracts to one line. The default status bar leads with the effective model and shows connection state, the selected Agent and its workspace. At phone widths, the workspace and usage move to a second row. Very short terminals keep a single row to leave room for your draft. Click a character in a draft to move the caret there, including on wrapped lines. Elapsed time appears directly beside the current activity.

Above the input, an activity mark and label show the selected Agent's state. The mark animates during active work and stays still when ready or finished. The display smooths brief phase changes and shows errors and requests for attention immediately. During work the Turn timer updates each second and keeps its start time across phases and Agent switches. The completed duration appears in the Turn summary. An input receipt clears once the Agent settles the Turn. Use `/statusline` to add the optional `execution` field to the footer. The composer shows its `To <agent>` recipient above the draft and grows as you type. `Draft` describes local text, while `Awaiting receipt`, `Input queued` and `Input accepted` describe submitted text.

`Waiting for model` shows a pending model request. Provider updates can move among `Thinking`, `Streaming reply` and `Writing tool arguments` as reasoning, visible reply text and tool arguments arrive. `Running tools` shows dispatched tools. `Waiting` identifies a pending question or permission, and `Failed` identifies an execution error. `Offline` indicates a service connection problem. `Updates unavailable` indicates that the live transcript subscription has failed. While reconnecting or missing updates, `Last seen` marks the most recent observed execution state.

In `/statusline`, use ↑ or ↓ to select a field, Space to toggle it, and ← or → to move an enabled field. Press R to preview the defaults, Enter to save, or Esc to return to the previous layout. Saving applies the layout and keeps it in the local `tui.toml` for later launches. If `KUKU_TUI_STATUSLINE` is set, its layout resumes when the editor closes. [TUI configuration](config.md#terminal-preferences)

| Field | How to read it |
|---|---|
| `Context …% used` | Input tokens reported for the latest measured request divided by the context window bound to that request. The value describes the last measured prompt. |
| `in`, `out` | Cumulative input and output tokens for this Agent across its requests. Input includes cached tokens. A `≥` prefix marks a partial total. |
| `Cache hit …%` | Total cache-read tokens divided by total input tokens across this Agent's requests that report both counts. `0%` means measured zero hits, and a dash means there is no positive measured input total. |
| `TPS` | Average output tokens per second for the selected Agent across completed requests with measured output and a positive duration. Divide their total output by their total duration, including first-token wait and excluding tools and retry backoff. |

A dash (`—`) means the measurement is unavailable. A newly prepared request clears the previous context percentage until its own usage arrives. Average TPS stays visible while the next request runs. To show only identity, directory and usage, set `KUKU_TUI_STATUSLINE=agent,workspace,context,tokens_in,tokens_out,cache,tps` before opening the terminal.

## `kuku run`

```text
kuku run [options] <prompt...>
```

Flags:

| Flag | Meaning |
|---|---|
| `-y`, `--yes` | Explicitly select the default trusted-local policy |
| `--connection <name>` | Exact configured connection name |
| `--model <id>` | Exact model identifier |
| `--tier <name>` | Reusable model route and settings |
| `--agent <address>` | Stable Session-local address; default `main` |
| `--command-id <id>` | Caller-selected identity for idempotent input retry |
| `--reasoning-effort <value>` | Provider-supported reasoning setting |
| `--service-tier <value>` | Provider-supported service setting |
| `--max-output-tokens <n>` | Maximum tokens for one model response |
| `-s`, `--session <id>` | Continue one session |
| `-c`, `--continue` | Continue the most recent session |
| `--json` | Emit one final JSON line |
| `--stream-json` | Emit realtime JSON lines |
| `--show-thinking` | Show public reasoning text or summaries when provided by the model service |
| `--raw` | Plain text output |
| `--verbose` | Show the durable Agent snapshot, configuration, and usage |
| `--prompts-dir <dir>` | Override embedded prompt assets when creating a Session. With `--remote`, use an authorized absolute directory on the server. |
| `--no-agents` | Disable `create_agent`, `send_message`, and `list_agents`; retain `ask_user` |
| `--no-skills` | Disable the default skill tool surface: `list_skills`, `search_skills`, and `use_skill`, plus slash-skill loading |

The server uses the SDK's trusted-local policy by default. After a run starts, Ctrl-C requests cancellation of the observed execution and waits for cleanup. Losing the client connection leaves the server's execution running. A fresh `run` can advance a stopped Agent. A waiting Agent requires its matching reply. Check the actual files or command results to verify task success.

Use `--tier` for a tier name. `--model` selects an exact identifier, and `--connection` selects one named connection even when multiple connections use the same protocol. An explicit `--model` overrides the tier’s model, including when both flags are supplied while reopening an Agent. Explicit settings affect only the selected Agent. Reopening without selection flags retains that Agent's configuration and independent context. A changed model route clears previous Agent-local reasoning/service overrides before applying new explicit values.

With `--command-id`, retrying the same input returns its original durable receipt and current Agent state. If a first send loses its reply, the client retains the generated Session ID with the unconfirmed command. After receiving its receipt, use the returned `--session` for further retries. `--json` and `--stream-json` report `event: "acknowledged"`, `replayed: true`, `receipt`, and `state` for a replay. Use a new command identity for new work or configuration.

`--json` emits one final `type: "session"` object with `event`, `session_id`, `agent`, `status`, `turns`, `response`, `error`, `config`, `usage`, `permission`, `question` and `duration_ms`. `--stream-json` also emits transient progress and committed-receipt information. Slow observers may lose transient deltas. The final response comes from committed state. Provider usage and unavailable reporting follow the [Session SDK](../how-it-works/sessions.md) contract.

If the prompt starts with `/skill-name` and the first token is a valid skill name, `kuku run` loads that Skill and sends the remaining text as the user prompt. Path-like prompts such as `/tmp/foo` stay plain user input. `--no-skills` disables that shortcut because it disables the default skill tool surface.

## `kuku reply`

```text
kuku reply <session-id> <request-id> [--agent <address>] (--allow | --deny | --answer <text>)
```

Replies target the exact retained request, including after reopening. `--allow` authorizes its bound action once; `--deny` rejects it; `--answer` answers a pending question. Ordinary `run` input never acts as approval. `--command-id` enables retrying the same reply; an already committed reply returns only its original acknowledgement and current state, without resuming work or issuing another permission decision. Stale or mismatched requests fail visibly. This command also accepts `--config`, `--workspace`, and `--data-dir`, observes the continuation, and awaits cleanup.

Reply output is JSON lines without a `--json` flag: a fresh question answer ends with an `AgentSnapshot`; a fresh permission reply first emits `permission_decision`, then the snapshot. A duplicate reply emits the `acknowledged` Session object with `receipt` and `state` described above. These replies do not use the normal `run` completion object.

## `kuku show`

```text
kuku show <session-id> [--agent <address>] [--json] [--workspace <path>] [--data-dir <path>]
```

Read the last durable response. `--json` returns the Agent snapshot, including any pending question or authorization identity. `show` and Session-specific `list` can query a Session while another client is working in it.

## `kuku events`

```text
kuku events [-v|-vv] [--agent <address>] [--after <id>] [--limit <n>] [--json] <session-id>
```

- `-v` or `-vv` increases the brief display detail.
- `--json` emits complete stored event records.
- `--after` is an exclusive durable cursor; `--limit` defaults to 100 and accepts 1–256 records. Filtering applies within that bounded ledger page. The last scanned cursor is printed to stderr, even when filtering excludes every record.
- `--workspace` and `--data-dir` select the stored Session location.
- `--conversation` remains an alias for `--agent` on inspection commands.

The server pages through committed `events.jsonl` facts. Observability logs live separately under `$KUKU_HOME/logs/`. [File layout](file-layout.md)

## `kuku list`

```text
kuku list [<session-id>] [--all] [--workspace <path>] [--data-dir <path>] [--verbose]
```

With a Session identity, `list` shows stable Agent addresses, statuses, and pending-input counts.

## `kuku delete`

```text
kuku delete [--workspace <path>] <session-id>
```

## `kuku pending`

```text
kuku pending list
kuku pending resend COMMAND_ID [--session SESSION_ID]
kuku pending discard COMMAND_ID [--session SESSION_ID]
```

`list` queries the selected service for receipts and removes confirmed entries from local pending storage. An unconfirmed entry can be resent with the original command identity and contents, or discarded from the client. Discarding leaves any already committed server work in place. Supply `--session` to distinguish entries with the same command ID in different Sessions.

## `kuku config`

```text
kuku config [--config <path>] [show|validate|set|policy]
```

Subcommands:

| Subcommand | Syntax |
|---|---|
| show | `kuku config show` |
| validate | `kuku config validate` |
| set | `kuku config set <key> <value>` |
| policy allow | `kuku config policy allow <risk>` |
| policy deny | `kuku config policy deny <risk>` |

`config validate` resolves each configured model tier with the same checks the service uses for new Sessions. It reports the tier that failed and does not call the provider.

`policy allow` and `policy deny` currently print a not-yet-implemented message instead of editing `policy.md`.

## `kuku prompts`

```text
kuku prompts [show [name] | export <dir>]
```

Valid `show` names:

- `system`
- `project-context`
- `tool-guidance`
- `runtime-context`

## `kuku agents`

```text
kuku agents [list | show <name>]
```

## `kuku skills`

```text
kuku skills [list | show <name>]
```

## `kuku server` and `kuku web`

```text
kuku server [--listen <addr>] [--config <path>] [--password <token>] [--max-concurrent-runs <n>]
kuku server status
kuku server restart
kuku web [--session <id> [--agent <address>]] [--print-url]
```

Defaults:

- `--listen 127.0.0.1:17777`
- `--max-concurrent-runs 16`

The explicit server requires `--password` or `KUKU_SERVER_PASSWORD`. `kuku web` opens the browser on the selected service. It can pass the workspace, Session and Agent selection to the page. Use `--print-url` to print the link. A managed sign-in link contains a short-lived, single-use ticket.

`server status` reports service information as JSON. For a stopped local service it returns `{"state":"stopped"}`. On a busy service, `server restart` exits with status 1 and emits JSON with `state: "deferred"` and the current `service` status. On success it reports `state: "restarted"` and the replacement service.

To build the embedded UI in a checkout, run these commands from the repository root with Node.js 24 and npm installed:

```bash
npm --prefix apps/web ci
npm --prefix apps/web run build
cargo run --locked -p kuku-app --features embedded-web-assets -- web
```

For service authentication and request formats, see [Run the server](../guides/run-the-server.md) and [Server API](server-api.md).
