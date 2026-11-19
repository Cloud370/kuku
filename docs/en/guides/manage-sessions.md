# Manage Sessions

Each session is stored as one directory and one ledger under kuku home.

In the terminal interface, use `/resume` to find a Session in the current workspace and `/agents` to switch recipients. Each window keeps its own draft. An accepted question or authorization reply becomes visible to other clients observing that Agent. [Terminal commands](../reference/cli.md#terminal-interface)

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

## Save and reuse a value

To save a long paste before the first model request, open `/save` in the terminal, paste the text, and give it a name. To keep a command log, open its capture with `/tools` and press S in the viewer. After reopening with `/resume`, use `/values` to read the value or attach its saved reference to your next input. [Terminal commands](../reference/cli.md#terminal-interface)

## Delete A Session

```bash
kuku delete <session-id>
```

If the session belongs to another workspace, add `--workspace`.

Closing a Session preserves its committed context and pending requests. While an input is queued, the interactive client can withdraw it. After model acceptance, the admitted context is immutable. [Sessions](../how-it-works/sessions.md)

## Related Pages

- [CLI](../reference/cli.md)
- [File Layout](../reference/file-layout.md)
- [Events](../reference/events.md)
