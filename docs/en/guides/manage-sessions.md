# Manage Sessions

Each session is stored as one directory and one ledger under kuku home.

## List Sessions

Current workspace only:

```bash
kuku list
```

All workspaces:

```bash
kuku list --all
```

One workspace explicitly:

```bash
kuku list --workspace /path/to/workspace
```

## List Agents In One Session

```bash
kuku list <session-id>
```

This shows each stable Agent address, execution status, and pending-input count.

## Continue A Session

Continue the `main` Agent:

```bash
kuku run --session <session-id> "continue"
```

Or continue the latest session:

```bash
kuku run --continue "continue"
```

Use `--agent reviewer` to create or continue another Agent in that Session. A new Agent starts with independent context; reopening an existing address retains its own context and model configuration.

A waiting Agent requires its exact pending request:

```bash
kuku reply <session-id> <request-id> --agent reviewer --answer "the requested information"
kuku reply <session-id> <request-id> --agent reviewer --allow
```

Use `--deny` to reject authorization. A normal follow-up prompt never grants permission. Reply command identities make retries idempotent; stale request identities are rejected.

## Read Session Data

```bash
kuku show <session-id>
kuku show <session-id> --agent review
kuku show <session-id> --agent review --json
kuku events <session-id>
kuku events <session-id> --agent review
```

- `kuku show` reads the last durable response for one Agent.
- `kuku events` reads the persisted ledger facts.
- omit `--agent` for full-ledger inspection.
- use `--after <id> --limit <n> --json` for a bounded durable event page; the default limit is 100.
- inspection accepts `--workspace` and `--data-dir` when the Session lives elsewhere.

## Delete A Session

```bash
kuku delete <session-id>
```

If the session belongs to another workspace, add `--workspace`.

Closing a Session preserves its committed context and pending requests. Inputs already admitted to a provider-visible context are immutable; the interactive host no longer offers in-place transcript rollback.

## Related Pages

- [CLI](../reference/cli.md)
- [File Layout](../reference/file-layout.md)
- [Events](../reference/events.md)
