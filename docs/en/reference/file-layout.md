# File Layout

## kuku Home

Default runtime home:

```text
~/.kuku/
```

Or, if set:

```text
$KUKU_HOME/
```

## Top-Level Layout

```text
$KUKU_HOME/
├── config.toml
├── credentials/
├── memory.md
├── packages/
├── agents/
├── skills/
├── logs/
└── p/
```

`logs/` stores observability logs. Session facts remain in each session's `events.jsonl`.

`credentials/` contains private, immutable plaintext key files created by interactive setup. Configs reference them with `api_key = "file:<absolute-path>"`; secrets are not copied into TOML or shown by `kuku config show`. Every workspace that uses the same data directory shares this credential store. Files remain accessible to processes running as the same user. On Unix it uses mode `0700`, and each key file uses `0600` (owner access only). On Windows, setup requires a protected ACL that grants access only to the current user. Symlinks and insecure existing paths are rejected rather than repaired. Files are not encrypted and are not automatically deleted when a config changes.

Rotating [provider diagnostics and full traces](environment-variables.md#provider-trace-logs) live under `$KUKU_HOME/logs/provider-trace/`.

## Project-Scoped Layout

kuku uses the resolved path of an existing workspace for its Sessions, project Memory and policy. Opening the same directory through a symbolic link addresses the same data. For `/code/my-app` the project home is shown below.

```text
$KUKU_HOME/p/code/my-app/
```

That directory may contain:

```text
$KUKU_HOME/p/<workspace>/
├── memory.md
├── policy.md
└── sessions/
```

## Session Layout

```text
<data-dir>/p/<workspace>/sessions/<session-id>/
├── events.jsonl
├── events.jsonl.lock
└── outputs/<output-id>.output
```

`events.jsonl` is authoritative. The OS lock protects a single writer, and its file can remain after close. `outputs/` holds bounded framed command captures, derived output and named value contents. The journal records naming and sharing separately from the stored bytes. The public SDK uses flat Agent addresses in the journal, without per-agent directories or hidden Agent aliases. Historical rollback backups can remain in older sessions. The Session API preserves admitted context.

The journal begins as JSON Lines. [Eligible Session closes](../how-it-works/sessions.md#when-the-journal-is-compressed) can seal history into independent gzip frames with an inline recovery checkpoint. New records append to a JSON Lines tail in the same file. Use `kuku events --file <path> --all --json` to stream the complete logical journal in either layout. [Session events](events.md)

See [Sessions](../how-it-works/sessions.md) for ownership and [SDK](sdk.md) for bounded reads and retention limits.

## Directory Creation Intents

On Unix, SDK directory creation may temporarily place an empty, owner-only `.kuku-mkdir-v1-<sha256>.intent` file in a directory's direct parent. The digest is SHA-256 of the child directory's raw filename bytes. The file lock serializes creation and recovery of that child; it is not a PID lease.

Before `mkdir`, the SDK syncs the intent file and its parent. It then syncs the created directory and its parent before removing the intent and syncing the parent again. Later creation or reopen checks the exact intent names along the requested path and completes interrupted transactions before descending. This also covers an interrupted first creation of a nested data directory. Existing directories without an intent need no directory sync, so unchanged ancestors need only traversal permission. A waiter whose locked intent was removed must re-open and lock the current inode before changing the directory.

Failed publication leaves the intent for recovery. Do not remove these files while SDK resources may be opening. Successful creation removes them. Windows file publication uses write-through file handles, full file flushes, and write-through moves; the Unix intent protocol does not apply there. Contract tests cover process exit and reopen, concurrent creators, replaced intent inodes, and directory permissions. They do not establish power-loss behavior on a particular filesystem or device.

## User and Project Extensions

Conventional user-level definitions:

- `~/.kuku/agents/`
- `~/.kuku/skills/`
- `~/.kuku/packages/`

Conventional project-level definitions inside a workspace:

- `<workspace>/.kuku/agents/`
- `<workspace>/.kuku/skills/`
- `<workspace>/.kuku/packages/`

With `auto_discover = true`, kuku also scans other user and project dot-directories for `skills/`, `agents/`, and `agent/` subdirectories. Compatibility layouts such as `.claude/skills`, `.claude/agents`, and `.opencode/agent` are discovered automatically.
