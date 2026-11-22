# Manage Sessions

In the TUI, use `/resume` to reopen a Session in the current workspace and `/agents` to pick which Agent receives your next message. Each window keeps its own draft. [Terminal interface](../reference/cli.md#terminal-interface)

## Rewind or branch from history

When an Agent is idle in the TUI, press **Ctrl+G** to open its conversation history. If you roll back from Turn 100 to Turn 90 and continue from there, opening the same list later still lets you jump back to the saved Turn 100 — both branches stay in the same Session. [History controls](../reference/cli.md#rewinding-and-branching)

## List Sessions and Agents

```bash
kuku list                                  # current workspace
kuku list --all                            # all workspaces
kuku list --workspace /path/to/workspace   # specific workspace
kuku list <session-id>                     # Agents in one Session (address, status, queued inputs)
```

## Continue a Session or reply to a waiting Agent

```bash
kuku run --session <session-id> "continue"
kuku run --continue "continue"
```

Pass `--agent reviewer` to create or address another Agent in the same Session. New Agents start with independent context; reopening an existing address keeps its own context and model settings.

When an Agent is waiting for an answer or permission, reply with its exact `request_id`:

```bash
kuku reply <session-id> <request-id> --agent reviewer --answer "the requested information"
kuku reply <session-id> <request-id> --agent reviewer --allow
```

Pass `--deny` to reject authorization. A regular follow-up prompt never grants permission. Replies carry a command ID so retries are idempotent, and stale request IDs are rejected.

## Inspect Session data

```bash
kuku show <session-id>
kuku show <session-id> --agent review
kuku show <session-id> --agent review --json
kuku events <session-id>
kuku events <session-id> --agent review
```

- `kuku show` prints the latest durable response for an Agent.
- `kuku events` shows the latest 40 committed records. Filter with `--agent` or `--kind`, or pass `--all --json` to export the full ledger. [Event inspection](../reference/cli.md#kuku-events)
- Both accept `--workspace` and `--data-dir` for Sessions stored elsewhere.

## Save and reuse values

To keep a long paste before the first model request, run `/save` in the TUI, paste the text, and name it. To keep a command log, open its capture with `/tools` and press **S** in the viewer. After reopening with `/resume`, use `/values` to read a saved value or attach its reference to your next message. [Terminal interface](../reference/cli.md#terminal-interface)

## Delete a Session

```bash
kuku delete <session-id>
```

Add `--workspace` if the Session belongs to another workspace.

Closing a Session preserves its committed context and pending requests. Queued inputs can be withdrawn while waiting in the queue, but become immutable once admitted into model context. [Sessions](../how-it-works/sessions.md) · [CLI](../reference/cli.md) · [File layout](../reference/file-layout.md) · [Events](../reference/events.md)
