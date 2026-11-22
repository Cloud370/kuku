# Manage Sessions

Use `/resume` to reopen a conversation in your current workspace. Use `/agents` to choose who receives your next message. Each window keeps its own draft. [Terminal commands](../reference/cli.md#terminal-interface)

## Pick up an earlier conversation

In the terminal, press Ctrl+G when the Agent is idle to open conversation history. If you go back from Turn 100 to Turn 90 and continue there, open the same list later and choose the saved Turn 100 to return. Both conversations are saved in the same Session. [Terminal controls](../reference/cli.md#edit-a-past-message-or-continue-from-history)

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
- `kuku events` shows the latest 40 committed records. Use `--agent` or `--kind` to find specific events. Use `--all --json` to export the ledger. [Event inspection](../reference/cli.md#kuku-events)
- inspection accepts `--workspace` and `--data-dir` when the Session lives elsewhere.

## Save and reuse a value

To save a long paste before the first model request, open `/save` in the terminal, paste the text, and give it a name. To keep a command log, open its capture with `/tools` and press S in the viewer. After reopening with `/resume`, use `/values` to read the value or attach its saved reference to your next input. [Terminal commands](../reference/cli.md#terminal-interface)

## Delete A Session

```bash
kuku delete <session-id>
```

If the session belongs to another workspace, add `--workspace`.

## Related Pages

- [CLI](../reference/cli.md)
- [File Layout](../reference/file-layout.md)
- [Events](../reference/events.md)
