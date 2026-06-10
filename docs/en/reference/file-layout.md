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
$KUKU_HOME/p/<workspace>/sessions/<session-id>/
├── lock
├── events.jsonl
└── pre-revert-<event-id>/
```

Notes:

- `lock` records the active writer.
- `events.jsonl` is the session ledger.
- `pre-revert-<event-id>/` appears when file rollback creates backups.

`subs/` is no longer the supported primary model for agent work. Canonical agent conversations live inside the same session ledger and are distinguished by conversation address in `events.jsonl`.

Older sessions may still contain compatibility artifacts from earlier delegated-layout experiments. Treat those as historical.

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
