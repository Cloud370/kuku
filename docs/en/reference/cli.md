# CLI

## Entry Modes

- `kuku` starts interactive mode in the current workspace.
- `kuku run ...` starts a non-interactive run.

## Top-Level Commands

| Command | Purpose |
|---|---|
| `kuku run <prompt...>` | Execute a task |
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
| `-y`, `--yes` | Auto-allow permission requests once |
| `--model <name>` | Tier name or bare model ID |
| `-s`, `--session <id>` | Continue one session |
| `-c`, `--continue` | Continue the most recent session |
| `--json` | Emit one final JSON line |
| `--stream-json` | Emit realtime JSON lines |
| `--show-thinking` | Show thinking content |
| `--raw` | Plain text output |
| `--verbose` | Show detailed run metrics (usage, tools, response) |
| `--config <path>` | Use a specific config file |
| `--prompts-dir <dir>` | Override embedded prompt assets |
| `--no-agents` | Disable the `agent` tool |
| `--no-skills` | Disable the default skill tool surface: `list_skills`, `search_skills`, and `use_skill`, plus slash-skill loading |

If the prompt starts with `/skill-name` and the first token is a valid skill name, `kuku run` loads that Skill and sends the remaining text as the user prompt. Path-like prompts such as `/tmp/foo` stay plain user input. `--no-skills` disables that shortcut because it disables the default skill tool surface.

## `kuku show`

```text
kuku show <session-id>
```

## `kuku events`

```text
kuku events [-v|-vv] <session-id>
```

- `-v` shows metadata
- `-vv` shows full context

This command reads the session `events.jsonl` fact log. Observability logs live separately under `$KUKU_HOME/logs/`; see [Sessions](../how-it-works/sessions.md#observability-logs).

## `kuku list`

```text
kuku list [--all] [--workspace <path>] [--verbose]
```

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
