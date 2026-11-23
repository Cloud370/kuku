# File layout

## kuku home

Default root is `~/.kuku/`, or `$KUKU_HOME/` when set:

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

- **`logs/`** — observability logs and rotating [provider traces](environment-variables.md#provider-trace-logs) under `logs/provider-trace/`. Session facts live only in each Session's `events.jsonl`.
- **`credentials/`** — owner-only plaintext key files written by interactive setup and referenced via `api_key = "file:<absolute-path>"`. Secrets are never copied into `config.toml` or printed by `kuku config show`. All workspaces sharing the same data directory share this folder (accessible to processes running as the same OS user). On Unix, the directory is `0700` and files are `0600`; on Windows, a protected ACL restricts access to the current user. Symlinks and insecure permissions are rejected rather than auto-repaired. Files are unencrypted and are not auto-deleted when config changes.

## Project-scoped layout

kuku keys Sessions, project Memory, and policy by the canonical path of the workspace (opening via a symlink hits the same data). For `/code/my-app`:

```text
$KUKU_HOME/p/code/my-app/
├── memory.md
├── policy.md
└── sessions/
```

## Session layout

```text
<data-dir>/p/<workspace>/sessions/<session-id>/
├── events.jsonl
├── events.jsonl.lock
└── outputs/<output-id>.output
```

- `events.jsonl` is the single source of truth; `events.jsonl.lock` holds the single-writer OS lock (the lock file may stay on disk after close).
- `outputs/` holds bounded framed command captures, derived outputs, and named values. Naming and sharing metadata live in the journal.
- Agents use flat addresses inside the journal with no per-Agent subdirectories or hidden aliases. (Older Sessions may still contain legacy rollback backups.)
- Journals start as plain JSONL. On [eligible clean closes](../how-it-works/sessions.md#when-the-journal-compacts), kuku can seal older records into independent gzip frames with an inline checkpoint while continuing to append JSONL to the tail. `kuku events --file <path> --all --json` reads both layouts transparently. [Session events](events.md) · [Sessions](../how-it-works/sessions.md) · [SDK](sdk.md)

## Directory creation intents (Unix)

When creating directories on Unix, the SDK may temporarily place an empty, owner-only `.kuku-mkdir-v1-<sha256>.intent` lock file in the parent directory (hashed from the child's raw filename bytes) to serialize creation and crash recovery. It syncs the intent and parent before `mkdir`, syncs the created directory and parent afterward, and removes the intent on success.

Do not delete `.intent` files while SDK resources may be opening — subsequent opens recover interrupted creations automatically. Windows uses write-through handles, flushes, and atomic moves instead of intent files.

## User and project extensions

Conventional locations:

- User: `~/.kuku/agents/`, `~/.kuku/skills/`, `~/.kuku/packages/`
- Project: `<workspace>/.kuku/agents/`, `<workspace>/.kuku/skills/`, `<workspace>/.kuku/packages/`

With `auto_discover = true`, kuku also scans other user and project dot-directories for `skills/`, `agents/`, and `agent/` (such as `.claude/skills`, `.claude/agents`, and `.opencode/agent`).
