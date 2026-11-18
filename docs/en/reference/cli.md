# CLI

## Entry Modes

- `kuku` opens the full-screen client in an interactive terminal.
- `kuku run ...` starts a non-interactive run. Use it in scripts or when no interactive terminal is available.

The terminal, script and browser clients submit work to the server. The local clients reuse the managed service for the selected data directory. [Run the server](../guides/run-the-server.md)

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
| `kuku init` | Initialize config and runtime directories |
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
| Ctrl+C | Clear the focused input. An empty editor stays open. |
| Enter | Send the draft. A busy Agent accepts it at the next safe model-request boundary, or in the next Turn if the current Turn settles first. |
| Alt+Enter | Queue the draft for the next Turn. |
| Shift+Enter or Ctrl+J | Insert a newline in the draft. |
| Esc | Close the focused menu, or interrupt the selected execution and keep the draft. |
| `/` in an empty composer, or F1 | Search available actions and Skills. Choosing a Skill inserts an editable `/skill:NAME` prefix for the next send. |
| `@` or Tab | Complete workspace-relative paths as draft text. |
| `/new`, `/resume` | Open a blank draft or find a retained Session. |
| `/agents` | Choose a recipient or create another Agent in the Session. |
| `/queue`, `/withdraw`, `/continue` | Queue for the next Turn, withdraw a queued input, or explicitly resume work. |
| F2 or `/reply` | Answer the selected Agent's pending question or authorization. |
| `/pending` | Check, retry or discard an unconfirmed submission. |
| `/model`, `/thinking` | Select model or reasoning settings for the selected Agent. |
| Ctrl+O, `/tools`, `/copy` | Tool details start expanded with a bounded preview. Toggle them with Ctrl+O, choose a saved result with `/tools`, or use `/copy` to copy the assistant’s reply. |
| Left-button drag | Highlight visible text and copy the selected range to the clipboard on release. |
| Page Up, Page Down, Ctrl+End | Scroll the transcript or return to the latest output. |
| Ctrl+P, Ctrl+N | Recall the previous or next sent prompt into the editor. |
| `/server`, `/reconnect`, `/restart` | Inspect the connected service, reconnect, or restart the managed local service. |
| Arrow keys, Tab, Enter in a menu | Move the highlight, complete editable text, or activate the selected action, respectively. |
| `/web`, `/quit` | Open the conversation in the browser or leave the terminal client. |

The session header identifies the conversation. Each Turn groups the user’s message, tool activity and assistant text. Markdown renders as the reply streams. The single-line footer shows the connection and selected Agent’s activity. The editor grows for wrapped or multiline input and shows draft and receipt feedback. `Draft` is local text. An input can be awaiting a receipt, queued or accepted. `Waiting for model` and `Streaming reply` describe the current model request. `Reconnecting` concerns the client’s connection to the service. Use `KUKU_TUI_STATUSLINE` to choose the visible fields and their order. [Environment variables](environment-variables.md)

The footer identifies the selected Agent by its Session-local address and shows the working directory reported by the service. Once the Agent has a snapshot, the default layout includes its usage measurements when space permits.

| Field | How to read it |
|---|---|
| `Context …% used` | Input tokens reported for the latest measured request divided by the context window bound to that request. The value describes the last measured prompt. |
| `in`, `out` | Cumulative input and output tokens for this Agent across its requests. Input includes cached tokens. A `≥` prefix marks a partial total. |
| `Cache hit …%` | Total cache-read input tokens divided by total input tokens for this Agent. A percentage requires complete measurements and a positive denominator. |
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
| `--show-thinking` | Show thinking content |
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
