# CLI

## Entry modes

- `kuku` opens the full-screen terminal interface (TUI).
- `kuku run ...` runs a non-interactive task for scripts or headless terminals.

The TUI, `kuku run`, and the web UI all talk to a kuku service. Locally, clients share one managed service per data directory. [Run the server](../guides/run-the-server.md)

First-run setup and `kuku init` share the same interactive local provider setup flow. `kuku init` requires a TTY, can edit an existing valid config, does not start the server, and does not configure remote servers. [Configuration](../start/configuration.md)

## Connection and workspace options

| Option | Meaning |
|---|---|
| `-w`, `--workspace <path>` | Workspace for file and command tools. Defaults to the resolved current directory locally; required as an absolute server-side path for remote Sessions. |
| `--data-dir <path>` | Data directory for the local service (defaults to `$KUKU_HOME` or `~/.kuku`). With `--remote`, selects the client's local storage directory. |
| `--config <path>` | Local config file path. Remote configs are managed on the server host. |
| `--remote <url>` | Connect explicitly to a remote server URL. |
| `--token <token>` | Bearer token for `--remote` (or set `KUKU_TOKEN`). |

For `kuku run`, place all options before the prompt — remaining words are treated as the prompt.

## Top-level commands

| Command | Purpose |
|---|---|
| `kuku run <prompt...>` | Submit a task and observe execution |
| `kuku reply <session-id> <request-id>` | Answer a durable question or permission request |
| `kuku show <session-id>` | Show a Session's final output |
| `kuku events <session-id>` | Inspect durable Session events |
| `kuku list` | List Sessions |
| `kuku pending ...` | Reconcile submissions with missing receipts |
| `kuku delete <session-id>` | Delete a Session |
| `kuku config ...` | Show, validate, or edit config |
| `kuku init` | Run local provider and credential setup |
| `kuku auth ...` | Sign in to and manage model subscriptions on the service host |
| `kuku prompts ...` | Inspect or export Prompt assets |
| `kuku agents ...` | List or inspect Agent presets |
| `kuku skills ...` | List or inspect Skills |
| `kuku server ...` | Run an explicit HTTP server |
| `kuku server status` | Inspect service identity and activity |
| `kuku server restart` | Restart the managed local service when idle |
| `kuku web ...` | Open the browser UI for the selected service |

## `kuku auth`

| Command | Purpose |
|---|---|
| `kuku auth login chatgpt [name]` | Add an account or reauthorize a saved account |
| `kuku auth list` | Show account names, masked emails, and sign-in status |
| `kuku auth models chatgpt <name>` | List the account's available models |
| `kuku auth proxy chatgpt <name> (--proxy URL \| --proxy-from-env \| --no-proxy)` | Change the saved account's network mode |
| `kuku auth logout chatgpt <name>` | Clear that account's tokens and attempt remote revocation |

Without a network flag, `login` uses the saved account setting or follows the environment for a new account. For proxy authentication, pass `--proxy-user` and either `--proxy-password-env` or `--proxy-password-file`. The password is read from the referenced source. [Headless sign-in](../start/configuration.md#sign-in-to-a-subscription-on-a-headless-host)

## Terminal interface

### Sending, editing, and stopping

| Action | How |
|---|---|
| Send immediately | Press `Enter`. If the Agent is running, it may pick up the message during the current Turn. |
| Queue for the next Turn | Press `Alt+Enter` to place the message in **Pending inputs**. |
| Edit a queued message | Click it, or press `↑` from an empty composer. `Enter` edits it; `Delete` removes it. |
| Interrupt the current Turn | Press `Esc` while the Agent is running. Output already shown stays visible. |

### Rewinding and branching

When the Agent is idle, press `Ctrl+G` or run `/rewind` (or `/jump`) to open conversation history. Active Turns and saved branches appear in chronological order with the latest Turn selected; scroll up to load older Turns.

Select a completed Turn and press `Enter` twice to switch back to that point (preserving its reply and tool results). If the composer is empty, the original prompt loads as a draft; existing drafts are kept. Select any **Saved** entry in the same list to return to a prior branch.

Workspace files stay unchanged by default. When rewinding to an earlier Turn on the current branch, press `Space` on the confirmation screen to preview restorable files and line diffs; `Enter` switches the conversation and applies those file changes. [How rewind works](../how-it-works/sessions.md#message-rewind)

### Keybindings and controls

| Control | Behavior |
|---|---|
| `Ctrl+C` | Clear the current draft (or exit TUI when the draft is empty; active server work continues). Inside an edit dialog, clears only that field. |
| `Shift+Enter` or `Ctrl+J` | Insert a newline in the draft. |
| `Esc` | Close an overlay, exit focused controls, or interrupt while running/cleaning up. Closing a reply editor preserves the reply draft. When idle with an empty composer, press `Esc` twice to open conversation history. |
| `Ctrl+P` or `F1` | Show read-only shortcut help without losing your draft. |
| `/` (on empty draft) or `Ctrl+P` then `/` | Search slash commands and Skills above the composer. Picking a Skill inserts an editable `/skill:NAME` prefix. |
| `@` or `Tab` | Complete workspace-relative file paths into the draft. |
| `Alt+←` / `Alt+→` | Cycle between Agents (`main` first, others sorted by address, wrapping at ends). |
| `F2` | Open tool approvals when any are waiting. Otherwise use the contextual control for questions, recovery, and other actions. |
| `F3` | Open workspace permission modes. |
| `Ctrl+T` | Cycle supported thinking levels for the active model (applies on the next safe request boundary). |
| **Repair credentials** | Shown after a matching auth failure to update the failing managed local connection and explicitly restart its service (remote connections show server-side instructions). [Recovering from an auth failure](../start/configuration.md#recover-an-authentication-failure) |
| **Messages** (narrow: **Msg**) | Inspect user, assistant, and tool entries in the current Agent's transcript. Pick an entry, then choose an action (`Full output` opens retained output or full inline content; multi-output JS scripts let you pick individual actions). |
| `Ctrl+O` | Expand or collapse visible transcript previews (tool groups and Agent cards). |
| `S` (in output viewer) | Name the selected retained output (`NAME --replace` overwrites an existing name). |
| Mouse drag | Highlight text and copy to clipboard on release. |
| Wheel, `Page Up` / `Page Down` | Scroll the transcript. |
| `Ctrl+End` or `Alt+↑` | Jump back to the latest output and resume auto-follow. |
| `↑` / `↓` in composer | Move across multi-line drafts, then scroll at edges. `Ctrl+↑` / `Ctrl+↓` browse sent prompt history (or queued inputs when focused on the queue). |
| `exit` or `quit` | Submitting a bare lowercase `exit` or `quit` with no attachments opens **Exit kuku?** (`Y` to exit, `Esc` to keep editing). |

Clicking rows in picker modals highlights them; clicking can also switch Agents/Sessions, open model input, or inspect messages and retained outputs. Applying model/thinking changes, permission decisions, or service restarts still requires `Enter`.

### Slash commands

Search matches both primary names and aliases; `Tab` completes the primary name and preserves arguments. `/reload` is reserved for config reload.

| Command | Aliases | Behavior |
|---|---|---|
| `/new` | `/new-session` | Open a fresh draft (previous Sessions remain accessible). |
| `/resume` | `/sessions` | Browse and switch to an existing Session in this workspace. |
| `/rewind` | `/backtrack`, `/jump` | Open conversation history and switch to a selected Turn. |
| `/compact` | `/compress`, `/summarize` | Compact older context for the selected Agent once idle (also `/compact retry` and `/compact undo`). |
| `/agents` | `/agent`, `/subagents` | Search Session Agents, switch recipients, or create an Agent. |
| `/jobs` | `/job`, `/ps` | List background Jobs for the selected Agent, read live output, or cancel (`/jobs cancel <id>`). |
| `/model` | `/models` | Enter switches to the highlighted route. `d` saves it as the default and switches, `e` edits it, and `/` searches. Ctrl+L opens the same list. |
| `/speed [tier]` | `/fast`, `/ultra`, `/ultrafast` | Choose the selected Agent's service tier above the conversation input. The shortcuts choose `fast` or `ultrafast`. Select **Follow model default** to clear the override. Type to filter the available tiers and press Tab to complete the selection. An unknown tier can be added with an optional short probe. |
| `/thinking` | `/think`, `/effort` | Pick a thinking level for the current Agent, choose **Use default** to clear the override, or press `D` + `Enter` on a writable local route to save as the model default. |
| `/statusline` | `/status-line` | Preview, toggle, reorder, and save status-line fields to `tui.toml`. |
| `/permissions [full\|ask\|read-only]` | `/perm`, `/permission` | Open the permission picker or apply a mode. |
| `/notifications` | `/notify`, `/sound` | Toggle and save terminal sounds. |
| `/save` | `/save-value` | Name pasted text or the current draft before sending. |
| `/values` | `/value`, `/saved-values` | Browse saved values to read or attach to the draft. |
| `/server` | `/service` | Inspect the connected service (`Ctrl+R` reconnects). |
| `/restart` | `/restart-service` | Confirm restarting the managed local service (deferred while busy). |
| `/web` | `/browser` | Open the conversation in the browser UI. |
| `/help` | `/hotkeys` | Show keyboard shortcuts. |
| `/quit` | `/exit`, `/q` | Exit the TUI (accepted work continues on the server). |

### Reading the transcript

- **Tool activity**: Consecutive workspace calls share a group. Its summary shows read and search counts, diagnostics, and each recorded file change with a short preview when available. Repeated diagnostics show once with a count. Select the heading to review each call or select a file to expand its preview in place. A minus marks a line in the old file and a plus marks a line in the new file. The header’s **Messages** view retains individual calls and full output. An open script shows **Input**, **Actions**, and **Output**. Input previews the JavaScript that the Agent submitted. Actions shows completion counts and the operations the script invoked. An object output shows its field count and up to three field names with their value summaries. Select **Input** or **Output** to expand the corresponding content in place.
- **Tool results**: Command results show the command, optional title, status, exit code, and parsed test counts. Use **View output** under an expanded command to open its saved output, including commands run inside `run_js`. Select **Read saved output** to open the byte range recorded for that read. An expanded file row shows the saved content or diff. Expanded `run_js` results show completed host actions, a failure reason when available, and links to saved output. The compaction heading shows the recorded duration and estimated input tokens before and after summarization.
- **Delegation & inbox**: Messages from another Agent display the sender (for example `From main`). Delegated Agent creation and task deliveries group into a delegation card with an **Open agent →** link and expandable details. Queued messages appear under **Pending inputs** until accepted.
- **Streaming & Markdown**: Markdown renders incrementally (including unclosed fences, task lists, nested quotes/lists, inline `<sup>`/`<sub>`/`<br>`, clickable HTTP/HTTPS links, and rendered `md`/`markdown` code fences). Interrupting a stream preserves a bounded preview in that Turn.
- **Thinking**: While the model reasons, an inline excerpt streams above the reply. Click **View thinking** (or choose **Thinking** in **Messages**) to open a scrollable 5-line inline reader; pre-tool reasoning is available via **Messages**.
- **Navigation & Turn stats**: Click **Jump to latest** to resume following; a per-Turn summary at the end of the reply shows elapsed time, tokens, cache hit rate, and tool count.

### Status and usage

The default bottom status bar shows the active model, resolved thinking level, and configured service tier, for example `gpt-6.2-sol xhigh ultrafast`, connection notices, Agent, workspace, context usage, cache hit rate, average TPS, total input and output tokens, and request cost. Pending settings show under **Next model request**.

If the next message would use a different model route or effective service tier from this Agent’s last request, the composer shows the change and warns that the cached prefix may miss. Press Enter again to send using the original send timing. Esc closes the notice and keeps the draft.

Above the composer, kuku displays the recipient (`To <agent>`), input delivery state (`Draft`, `Awaiting receipt`, `Input queued`, `Input accepted`), live elapsed Turn timer, and Agent execution state:

- `Waiting for model`, `Thinking`, `Streaming reply`, `Writing tool arguments`, `Running tools`
- `Waiting` (pending question or permission), `Failed` (execution error)
- `Offline` (connection lost), `Updates unavailable` (live stream failed), with `Last seen` showing the last observed state

Customize fields interactively with `/statusline` (writes `tui.toml`) or per-launch with `KUKU_TUI_STATUSLINE`. [Terminal preferences](config.md#terminal-preferences)

| Metric | Meaning |
|---|---|
| `Context` | Input tokens of the latest measured request divided by its context window. With sufficient width, the default layout also shows the input token count and window size. |
| `in`, `out` | Cumulative input (including cache) and output tokens across the Agent's requests. `≥` means only some requests reported usage. |
| `Cache hit …%` | Cumulative cache-read tokens divided by cumulative input tokens across requests reporting both. `0%` is measured zero; `—` means no positive measured input yet. |
| `TPS` | Total output tokens divided by total request duration across completed requests with output and valid timing (includes time-to-first-token; excludes tool execution and retry backoff). |
| Request cost | Session request cost in USD. `~` marks a total that includes estimates. `≥` marks a lower bound. |

## `kuku run`

```text
kuku run [OPTIONS] <PROMPT...>
```

| Option | Meaning |
|---|---|
| `-s`, `--session <id>` | Continue an existing Session |
| `-a`, `--agent <name>` | Target a specific Agent (defaults to `main`) |
| `-m`, `--model <spec>` | Override model route for this run |
| `--json` | Print one JSON object per line |
| `-o`, `--output <path>` | Write final output to a file |
| `--no-wait` | Return after the service accepts the input |
| `--auto-approve` | Allow permission requests during this run |
| `--wait-for-reply` | Stay attached while a durable request awaits an external reply |
| `--timeout <seconds>` | Exit after this wait timeout without canceling server work |

With `--json`, `kuku run` emits `session`, `accepted`, `delta`, `tool.call`, `tool.result`, `request`, `turn.completed`, `turn.failed`, and `error` lines. Wait timeouts exit with code `124`. [Run a task](../guides/run-a-task.md)

## `kuku reply`

```text
kuku reply <SESSION_ID> <REQUEST_ID> --allow
kuku reply <SESSION_ID> <REQUEST_ID> --deny
kuku reply <SESSION_ID> <REQUEST_ID> --answer "<text>"
kuku reply <SESSION_ID> <REQUEST_ID> --answer-file <path>
kuku reply <SESSION_ID> <REQUEST_ID> --answer-stdin
```

Answers a durable permission request or question. Returns cleanly if the same reply was already recorded.

## `kuku show`

```text
kuku show <SESSION_ID> [--agent <name>] [--json]
```

Prints the final assistant reply for the Session or selected Agent.

## `kuku events`

```text
kuku events <SESSION_ID> [--agent <name>] [--kind <kind>] [--after <id> | --before <id>] [--limit <n>] [--all] [--json]
kuku events --file <EVENTS_JSONL> [--agent <name>] [--kind <kind>] [--after <id> | --before <id>] [--limit <n>] [--all] [--json]
```

Prints durable Session events. `--json` writes one record per line to `stdout` and prints pagination flags to `stderr`. `--file` inspects a local journal offline (transparently reading sealed history and open tails). [Session events](events.md)

## `kuku list`

```text
kuku list [--json]
```

Lists Sessions for the selected workspace.

## `kuku pending`

```text
kuku pending list [--json]
kuku pending reconcile [--id <record-id>] [--json]
kuku pending drop <record-id> [--json]
```

Inspects and reconciles submissions whose HTTP receipt was lost due to a timeout or disconnect.

## `kuku delete`

```text
kuku delete <SESSION_ID> [--force]
```

Deletes a Session.

## `kuku config`

```text
kuku config path
kuku config show
kuku config validate
kuku config set <key> <value>
kuku config init
```

`kuku config show` redacts API keys. `kuku config set` updates scalar fields (for example `default_model`, `provider.<name>.model_pricing.<model>.mode`, or `provider.<name>.proxy.mode`). `kuku config init` writes the starter config file (non-interactive). Use `kuku init` for interactive provider and credential setup.

## `kuku prompts`

```text
kuku prompts list [--json]
kuku prompts show <name> [--json]
kuku prompts export <dir>
```

Inspects or exports the built-in Prompt assets.

## `kuku agents` and `kuku skills`

```text
kuku agents list
kuku agents show <name>
kuku skills list
kuku skills show <name>
```

Lists and inspects discovered Agent presets and Skills.

## `kuku server` and `kuku web`

```text
kuku server [--host <addr>] [--port <port>] [--password <secret>]
kuku server status
kuku server restart
kuku web [--session <id>] [--agent <name>] [--print-url]
```

- `kuku server` requires `--password` or `KUKU_SERVER_PASSWORD`.
- `kuku web` opens the browser UI on the selected service (managed sign-in links carry a short-lived, single-use ticket; use `--print-url` to print without opening).
- `kuku server status` returns service info as JSON (`{"state":"stopped"}` when local service is down).
- `kuku server restart` exits `1` with `state: "deferred"` when busy, or `0` with `state: "restarted"` on success.

To build the embedded web UI from a source checkout (requires Node.js 24 and npm):

```bash
npm --prefix apps/web ci
npm --prefix apps/web run build
cargo run --locked -p kuku-app --features embedded-web-assets -- web
```

[Run the server](../guides/run-the-server.md) · [Server API](server-api.md)
