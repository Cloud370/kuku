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
├── memory.md
├── packages/
├── agents/
├── skills/
├── logs/
└── p/
```

`logs/` stores observability logs. Session facts remain in each session's `events.jsonl`.

When provider tracing is enabled with `KUKU_PROVIDER_TRACE=1`, request and response diagnostics are written under:

```text
$KUKU_HOME/logs/provider-trace/<yyyy-mm-dd>/<session-id>.jsonl
```

## Project-Scoped Layout

For workspace `/code/my-app`, the project home is:

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

`events.jsonl` is authoritative. The OS lock protects a single writer; its file can remain after close. `outputs/` retains bounded framed stdout/stderr for the session lifetime. The public SDK uses flat Agent addresses in the journal, without per-agent directories or hidden Agent aliases. Historical rollback backups can remain in older sessions; the new Session API does not rewrite admitted context in place.

See [Sessions](../how-it-works/sessions.md) for ownership and [SDK](sdk.md) for bounded reads and retention limits.

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
