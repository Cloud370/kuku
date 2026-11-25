# CLI

## Entry modes

- `kuku` opens the full-screen terminal interface (TUI).
- `kuku run ...` runs a non-interactive task for scripts or headless terminals.

The TUI and `kuku run` talk to a kuku service. Local clients share one managed service per data directory. [Run the server](../guides/run-the-server.md)

Run `kuku` to set up a first connection and model interactively. For headless setup, use [`kuku config set --patch`](#kuku-config). [Configuration guide](../start/configuration.md)

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
| `kuku run ...` | Submit text or a prompt file and observe execution |
| `kuku reply <session-id> <request-id>` | Answer a durable question or permission request |
| `kuku show <session-id>` | Show a Session's final output |
| `kuku events <session-id>` | Inspect durable Session events |
| `kuku list` | List Sessions |
| `kuku pending ...` | Reconcile submissions with missing receipts |
| `kuku delete <session-id>` | Delete a Session |
| `kuku config ...` | Show, validate, or edit config |
| `kuku auth ...` | Sign in to and manage model subscriptions on the service host |
| `kuku prompts ...` | Inspect or export Prompt assets |
| `kuku agents ...` | List or inspect Agent presets |
| `kuku skills ...` | List or inspect Skills |
| `kuku server ...` | Run an explicit HTTP server |
| `kuku server status` | Inspect service identity and activity |
| `kuku server restart` | Restart the managed local service when idle |

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

Use your terminal’s usual paste shortcut in the composer. When the terminal forwards `Ctrl+V` or `Ctrl+Shift+V` as a key, kuku reads the local clipboard. An empty bracketed paste event also triggers a local image lookup. For terminals that handle the shortcut and forward only nonempty text, `Ctrl+Alt+V` can trigger the clipboard lookup directly. A pasted image appears as a highlighted `[Image #1]` in the draft. The cursor moves across the marker in one step. Press Backspace or Delete next to it to remove the whole attachment. Text paste continues to use the terminal's usual shortcut. A paste containing a single readable PNG or JPEG file path attaches that image too. On WSL, kuku resolves Windows drive paths through the mounted drive. An attachment hint above the composer shows the image count and whether PNG preserves transparency.

Press `Enter` to review the image quality above the draft. **Compressed** is selected initially and sends after three seconds without input. Compressed images use JPEG with quality 85, or PNG when there are transparent pixels. Choose **Original** with `Tab`, the arrow keys, or a click to preserve the source dimensions and pixels. Interacting with the picker cancels the countdown. Press `Enter` to send your selection or `Esc` to keep editing. Each submitted image must fit 1 MiB, with up to four images per message.

An image stays in the draft until you send it. When the selected model cannot accept images, kuku keeps the draft so you can switch models or remove the attachments.

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
| `Ctrl+P` or `F1` | Show shortcut help and how to ask about kuku. The draft stays open. |
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
| `/model` | `/models` | Enter selects the highlighted connection and model for the current Agent. `d` opens confirmation to save it as the default for new Sessions, `e` edits it, and `/` searches. `Ctrl+L` opens the same list. |
| `/speed [tier]` | `/fast`, `/ultra`, `/ultrafast` | Choose the selected Agent's service tier above the conversation input. The shortcuts choose `fast` or `ultrafast`. Select **Follow model default** to clear the override. Type to filter the available tiers and press Tab to complete the selection. An unknown tier can be added with an optional short probe. |
| `/thinking` | `/think`, `/effort` | Pick a thinking level for the current Agent, choose **Use default** to clear the override, or press `D` + `Enter` on a writable local route to save as the model default. |
| `/statusline` | `/status-line` | Preview, toggle, reorder, and save status-line fields to `tui.toml`. |
| `/permissions [full\|ask\|read-only]` | `/perm`, `/permission` | Open the permission picker or apply a mode. |
| `/notifications` | `/notify`, `/sound` | Toggle and save terminal sounds. |
| `/save` | `/save-value` | Name pasted text or the current draft before sending. |
| `/values` | `/value`, `/saved-values` | Browse saved values to read or attach to the draft. |
| `/server` | `/service` | Inspect the connected service (`Ctrl+R` reconnects). |
| `/restart` | `/restart-service` | Confirm restarting the managed local service (deferred while busy). |
| `/help` | `/hotkeys` | Show keyboard shortcuts and how to ask about kuku. |
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
kuku run [OPTIONS] --prompt-file <PATH|->
```

| Option | Meaning |
|---|---|
| `--prompt-file <PATH\|->` | Read the entire UTF-8 prompt from a file or from stdin with `-`. Supply one prompt source. |
| `-s`, `--session <id>` | Continue an existing Session |
| `-c`, `--continue` | Continue the most recent Session |
| `--agent <address>` | Target a specific Agent (defaults to `main`) |
| `--preset <name>` | Create the Agent from a preset in a new Session |
| `--connection <name>` and `--model <id>` | Select a configured pair by exact ID. Pass both options together. |
| `--reasoning-effort <value>` | Override native reasoning for the selected model |
| `--service-tier <value>` | Override the provider's native service option |
| `--max-output-tokens <n>` | Cap output tokens per model request |
| `--command-id <id>` | Give a submission a stable identity for an exact retry |
| `--json` | Write one final JSON result to stdout |
| `--stream-json` | Write live events and a final result to stdout as JSON Lines |
| `--raw` / `--verbose` | Select plain text or detailed human-readable output |
| `--show-thinking` | Include reported thinking while observing |
| `--prompts-dir <path>` | Override bundled Prompt assets |
| `--no-agents` / `--no-skills` | Disable delegation tools or Skill tools and loading |

File and stdin prompts are passed as text, including newlines. Use a positional prompt for explicit [Skill invocation](../guides/use-skills.md#use-skills-during-a-run).

The final object has `type: "session"`, a `status` and `event` describing the outcome, and the `session_id` and `agent` when available. A completed run includes its final `response`, a failed run includes its `error`, and available route and usage information appears in `connection`, `model`, and `usage`. A waiting result identifies its `question` or `permission` by `id`. Diagnostics go to stderr. Argument errors can exit before there is a Session result.

The receipt identifies the submitted input. Once the Agent admits it, streamed output and the final result belong to that Turn, even if another client starts a later Turn. The result reports that Turn's usage when available. Withdrawing or replacing the input while it is queued ends the run with `status: "cancelled"` and `input_status: "withdrawn"`.

Retrying a committed `--command-id` returns its original `receipt`, `replayed: true`, and the current `input_status` when known. Use `kuku show <SESSION_ID> --json` to read the Agent's current state.

| Exit code | Result status | Meaning |
|---|---|---|
| `0` | `completed` | The run completed. |
| `0` | `acknowledged` | The submission already has a durable receipt. |
| `3` | `waiting` | An Agent needs an answer or permission. |
| `1` | `failed` | The run failed. |
| `130` | `cancelled` | The run was cancelled. |
| `2` | | Command-line arguments were invalid. |

[Run a task](../guides/run-a-task.md)

## `kuku reply`

```text
kuku reply <SESSION_ID> <REQUEST_ID> --allow
kuku reply <SESSION_ID> <REQUEST_ID> --deny
kuku reply <SESSION_ID> <REQUEST_ID> --answer "<text>"
```

Answers a durable permission request or question and observes the continuation. Pass `--agent <address>` for a request from an Agent other than `main`. Use `--command-id <id>` to retry the same reply. A new reply reports the Turn that admitted the answer or resumed the permission request. A reply to a detached script permission reports its Job. Ctrl-C targets that continuation. An exact retry returns the original acknowledgement and the current Agent snapshot in `state`.

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
kuku list [-a | --all] [--json]
kuku list <SESSION_ID> [--json]
```

Lists Sessions for the selected workspace or all workspaces with `--all`. Pass a Session ID to list its Agents.

## `kuku pending`

```text
kuku pending list
kuku pending resend <COMMAND_ID> [--session <id>]
kuku pending discard <COMMAND_ID> [--session <id>]
```

Inspects submissions whose HTTP receipt was lost due to a timeout or disconnect. `list` checks for receipts. If a command is still unconfirmed, `resend` retries its original intent and `discard` removes the local pending record.

## `kuku delete`

```text
kuku delete <SESSION_ID>
```

Deletes a Session after closing it if needed. Active executions, cleanup, and Jobs prevent deletion.

## `kuku config`

```text
kuku config show [--json]
kuku config models [--discover --connection NAME]
kuku config validate
kuku config test --connection NAME --model ID [--check text|image|tools] [--reasoning-effort LEVEL]
kuku config set <KEY> <VALUE>
kuku config set --patch <TOML>
```

These commands select the local file in this order.

1. The path passed with `--config PATH`.
2. The running managed service's config path for this data directory.
3. The data directory's `config.toml`.

`show` writes redacted TOML to stdout and the path to stderr. Pass `--json` to get the TOML in the `config` field of a JSON result. The other commands report JSON by default. This includes `path`, `status`, `diagnostics` with any relevant field name, and `service.state`. Check for `valid` after validation and `updated` after a write.

`validate` checks the local file and its model settings without making a provider request. Read `service.state` to see whether the current file version is `loaded`, `pending_reload`, `different_path`, `stopped`, or `unknown`. Config commands can run while the service is stopped. Repeat validation to confirm the loaded version. [Configuration](../start/configuration.md)

`models` lists configured connection and model pairs. Each row contains `selection` or an `error` for that pair. With `--discover --connection NAME`, it queries that connection's model-list endpoint and returns the discovered exact IDs in `models[].id`.

`test` runs the same synthetic checks as interactive setup, using one exact registered connection and model. `--check text` (the default) or `--check image` sends one model request. `--check tools` sends up to two: one to request a synthetic tool call and, if the model returns it, another with its result. These requests can incur provider charges. A `tested` status means the probe returned evidence, not that the advertised capability passed; check `evidence.text_returned`, `evidence.image_match`, or `evidence.tool_round_trip` respectively.

Text tests use the configured reasoning setting unless you give `--reasoning-effort LEVEL`. Probes use their own output budget, not the Agent's output limit. Try a draft with `kuku --config ./draft.toml config test --connection NAME --model ID`; the command neither writes the configuration nor starts the service.

When the managed service advertises a config command to its Agents, it gives the executable's absolute path and a required CLI build version. An older CLI or one with a different reported build will reject the guarded invocation before dispatch. This checks the reported version, not binary integrity. If the CLI is unavailable or no managed CLI is advertised, use a verified CLI on the service host rather than guessing a binary from `PATH`.

`set KEY VALUE` updates one scalar. Quote a TOML dotted path that contains a quoted model ID, for example `kuku config set 'model.work."your-model-id".max_output_tokens' 8192`. Use `--patch` for a complete change or to create a missing file. Pass TOML text as one argument.

The optional top-level `delete` array lists TOML dotted paths to remove before merging the patch. For example, this patch removes a model registration while replacing the default with another configured pair.

```toml
delete = ['model.work."your-model-id"']
default_model = { connection = "work", model = "another-model-id" }
```

Use `provider.NAME` to delete a connection and remove its dependent model registrations or replace them in the same patch. A patch merges nested tables and replaces `default_model` as a complete pair. The command validates the whole resulting file before publishing it. An invalid patch or a conflicting write returns a nonzero exit status and a diagnostic. [Minimal config](config.md#minimal-config) · [Headless setup](../guides/run-a-task.md#set-up-from-a-shell)

## `kuku prompts`

```text
kuku prompts
kuku prompts show [NAME]
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

## `kuku server`

```text
kuku server [--listen <addr:port>] [--password <secret>] [--max-concurrent-runs <n>]
kuku server status
kuku server restart [--config PATH]
```

- `kuku server` requires `--password` or `KUKU_SERVER_PASSWORD`.
- `kuku server status` returns service info as JSON (`{"state":"stopped"}` when local service is down).
- `kuku server restart` exits `1` with `state: "deferred"` when busy, or `0` with `state: "restarted"` on success.

[Run the server](../guides/run-the-server.md) · [Server API](server-api.md)
