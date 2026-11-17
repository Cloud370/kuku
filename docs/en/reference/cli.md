# CLI

## Entry Modes

- `kuku` starts interactive mode in the current workspace.
- `kuku run ...` starts a non-interactive run.

## Top-Level Commands

| Command | Purpose |
|---|---|
| `kuku run <prompt...>` | Execute a task through the public Session SDK |
| `kuku reply <session-id> <request-id>` | Answer a durable question or authorization |
| `kuku show <session-id>` | Show final output from a session |
| `kuku events <session-id>` | Show persisted session facts |
| `kuku list` | List sessions |
| `kuku delete <session-id>` | Delete a session |
| `kuku config ...` | Show, validate, or change config |
| `kuku init` | Initialize config and runtime directories |
| `kuku prompts ...` | Show or export prompt assets |
| `kuku agents ...` | List or inspect agents |
| `kuku skills ...` | List or inspect skills |
| `kuku server ...` | Start the HTTP API server |
| `kuku web ...` | Start the HTTP server with embedded Web UI |

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
| `--workspace <path>` | Working directory; default current directory |
| `--data-dir <path>` | Persistent data; default `$KUKU_HOME` or `~/.kuku` |
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
| `--config <path>` | Use a specific config file |
| `--prompts-dir <dir>` | Override embedded prompt assets |
| `--no-agents` | Disable the `agent` tool |
| `--no-skills` | Disable the default skill tool surface: `list_skills`, `search_skills`, and `use_skill`, plus slash-skill loading |

The local CLI uses the SDK's trusted-local policy. It does not require `--yes` to run file or command tools. The Session owns execution; Ctrl-C cancels the Agent and waits for cleanup, and every run awaits asynchronous Session close. The local Harness lets already-running Agents settle before normal close; it does not automatically resume waiting or interrupted Agents. A completed Turn records execution completion, while task success still needs a file or command verifier.

Use `--tier` for a tier name. `--model` selects an exact identifier, and `--connection` selects one named connection even when multiple connections use the same protocol. Explicit settings affect only the selected Agent. Reopening without selection flags retains that Agent's configuration and independent context. A changed model route clears previous Agent-local reasoning/service overrides before applying new explicit values.

`--json` emits one final `type: "session"` object with `event`, `session_id`, `agent`, `status`, `response`, `config`, `usage`, `permission`, and `question`. `--stream-json` also emits transient progress, committed-receipt information, and a final durable result. Slow observers may lose transient deltas; the final response comes from committed state. Provider usage and unavailable reporting follow the [Session SDK](../how-it-works/sessions.md) contract.

If the prompt starts with `/skill-name` and the first token is a valid skill name, `kuku run` loads that Skill and sends the remaining text as the user prompt. Path-like prompts such as `/tmp/foo` stay plain user input. `--no-skills` disables that shortcut because it disables the default skill tool surface.

## `kuku reply`

```text
kuku reply <session-id> <request-id> [--agent <address>] (--allow | --deny | --answer <text>)
```

Replies target the exact retained request, including after reopening. `--allow` authorizes its bound action once; `--deny` rejects it; `--answer` answers a pending question. Ordinary `run` input never acts as approval. `--command-id` enables retrying the same reply; stale or mismatched requests fail visibly. This command also accepts `--config`, `--workspace`, and `--data-dir`, observes the continuation, and awaits cleanup.

## `kuku show`

```text
kuku show <session-id> [--agent <address>] [--json] [--workspace <path>] [--data-dir <path>]
```

Read the last durable response without opening an execution runtime. `--json` returns the complete Agent snapshot, including pending question/authorization identities.

## `kuku events`

```text
kuku events [-v|-vv] [--agent <address>] [--after <id>] [--limit <n>] [--json] <session-id>
```

- `-v` or `-vv` increases the brief display detail.
- `--json` emits complete stored event records.
- `--after` is an exclusive durable cursor; `--limit` defaults to 100 and accepts 1–256 records. Filtering applies within that bounded ledger page. The last scanned cursor is printed to stderr, even when filtering excludes every record.
- `--workspace` and `--data-dir` select the stored Session location.
- `--conversation` remains an alias for `--agent` on inspection commands.

This command reads the session `events.jsonl` fact log. Observability logs live separately under `$KUKU_HOME/logs/`; see [Sessions](file-layout.md).

## `kuku list`

```text
kuku list [<session-id>] [--all] [--workspace <path>] [--data-dir <path>] [--verbose]
```

With a Session identity, `list` shows stable Agent addresses, statuses, and pending-input counts.

## `kuku delete`

```text
kuku delete [--workspace <path>] <session-id>
```

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
```

Defaults:

- `--listen 127.0.0.1:17777`
- `--max-concurrent-runs 16`

`kuku web` requires a binary built with embedded Web UI assets. The release workflow and release artifacts include them. For local source builds, build `kuku-app` with `--features embedded-web-assets` after preparing `apps/web/dist`.

For request and stream formats, see [Server API](server-api.md).
