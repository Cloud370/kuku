# Use Agents

Agents are contact cards. When kuku calls one, it opens or continues a delegated conversation address inside the current session.

## Add an Agent

Place agent files in one of these locations:

- `~/.kuku/agents/`
- `<workspace>/.kuku/agents/`

These are the conventional locations. With auto-discovery enabled, kuku also scans other user and project dot-directories for both `agents/` and `agent/`, such as `.claude/agents` and `.opencode/agent`.

Project agents override user agents with the same name.

The file format is defined in [Agent Format](../reference/agent-format.md).

## Check Discovery

```bash
kuku agents list
kuku agents show <name>
```

If an agent does not appear, review your `[discovery]` settings in [Config](../reference/config.md).

## Use Agents in a Run

When the agent tool is enabled, kuku can delegate part of the work to discovered agent contacts.

Think in addresses:

- `review` means one ongoing review thread
- `review/api` means a separate nested thread rooted at the `review` contact
- reusing `review` means continuity

The `main` address is reserved for the host conversation and cannot be used as an agent target.

Passing a model tier only makes sense when first opening a new address. If you pass a tier while continuing an existing address, the runtime rejects the call.

To disable agent delegation for one run:

```bash
kuku run --no-agents "task"
```

## Choose User vs Project Scope

- Use `~/.kuku/agents/` for personal reusable contacts.
- Use `<workspace>/.kuku/agents/` when the contact belongs to one repository.

## Inspect Conversations

```bash
kuku list <session-id>
kuku show <session-id> --conversation review
kuku events <session-id> --conversation review
```

- `kuku list <session-id>` lists conversation addresses in one session.
- `kuku show` shows the final transcript output for one conversation.
- `kuku events` shows the underlying ledger facts, optionally filtered by conversation.

## Related Pages

- [Tools](../reference/tools.md)
- [Manage Sessions](manage-sessions.md)
