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

## List Conversations In One Session

```bash
kuku list <session-id>
```

This shows each conversation address, its active binding, and whether its latest turn is open, active, completed, cancelled, or interrupted.

## Continue A Session

Continue the main conversation:

```bash
kuku run --session <session-id> "continue"
```

Or continue the latest session:

```bash
kuku run --continue "continue"
```

Agent conversations continue by reusing the same conversation address through the `agent` tool.

## Read Session Data

```bash
kuku show <session-id>
kuku show <session-id> --conversation review
kuku events <session-id>
kuku events <session-id> --conversation review
```

- `kuku show` reads the final transcript output for one conversation.
- `kuku events` reads the persisted ledger facts.
- omit `--conversation` for full-ledger inspection.
- add `-v` or `-vv` to `kuku events` for more detail.

## Delete A Session

```bash
kuku delete <session-id>
```

If the session belongs to another workspace, add `--workspace`.

## Roll Back A Conversation

In interactive mode, use `/undo`.

Rollback is conversation-scoped in the canonical model. It appends rollback facts instead of deleting history.

## Related Pages

- [CLI](../reference/cli.md)
- [File Layout](../reference/file-layout.md)
- [Events](../reference/events.md)
