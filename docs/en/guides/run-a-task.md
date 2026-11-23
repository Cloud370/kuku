# Run a task

## One-shot runs

Run a task from any directory — no Git repository is required:

```bash
kuku run "check this project"
kuku run --tier strong "review this diff"
kuku run --connection <configured-name> --model <model-id> "task"
```

Pass `--workspace <path>` to target another directory. The default local policy allows file and command tools, so inspect changed files and command results before treating a task as done. Full flags are in [CLI reference](../reference/cli.md).

## Interactive TUI

```bash
kuku
```

Type a prompt and press **Enter** to send. While an Agent is running:

- **Enter** sends a follow-up that takes effect at the next safe model-request boundary (or starts a new Turn if the current Turn finishes first).
- **Alt+Enter** queues the message for the next Turn.
- **Shift+Enter** or **Ctrl+J** inserts a newline.
- **Esc** interrupts the selected Agent while keeping your draft (in a menu, **Esc** closes the menu).

Use `/new` for a fresh Session draft, `/resume` to reopen a Session in the current workspace, and `/agents` to pick which Agent receives your next message. Closing the terminal leaves accepted work running in the local service. [Terminal controls](../reference/cli.md#terminal-interface)

## Continue existing work

```bash
kuku run --session <session-id> "continue"
kuku run --continue "continue"
```

## Inspect output

- `kuku show <session-id>` — latest durable answer
- `kuku events <session-id>` — persisted event log
- `kuku list` — recent Sessions in the current workspace

## Machine-readable and verbose modes

- `--json` — one final JSON object with durable state and provider-reported metrics
- `--stream-json` — live JSON lines of transient progress, ending with the same final object
- `--verbose` — human-readable output with state, settings, and usage
- `--raw` — plain text only

Field schemas and request options live in [CLI reference](../reference/cli.md#kuku-run). Usage counters come from the provider; a missing field does not mean zero usage.

When an Agent pauses for a question or permission, `--json` / `--stream-json` ends with `event: "waiting"` and the exact `request_id`. Answer it with `kuku reply <session-id> <request-id> --answer "..."`, `--allow`, or `--deny` using the same workspace and data directory. [Manage Sessions](manage-sessions.md)

Pressing `Ctrl-C` waits for owned child commands to clean up. The Session log and retained outputs survive closing and reopening. [File layout](../reference/file-layout.md) · [Events](../reference/events.md)
