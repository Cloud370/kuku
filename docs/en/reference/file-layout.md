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
└── p/
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
├── pre-revert-<event-id>/
└── subs/
```

Notes:

- `lock` records the active writer.
- `events.jsonl` is the canonical session log.
- `pre-revert-<event-id>/` appears when file rollback creates backups.
- `subs/` is used when child sessions are present.

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
