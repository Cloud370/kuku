# Run a Task

Use this flow when kuku is already installed and configured.

## Start a New Task

For a one-shot run:

```bash
kuku run "check this project"
```

To choose a tier explicitly:

```bash
kuku run --tier strong "review this diff"
```

For an exact endpoint and model, use `kuku run --connection <configured-name> --model <model-id> "task"`. Run in an ordinary directory or add `--workspace <path>`; no Git initialization is required. The default local policy allows file and command tools. Verify the changed files and command results before treating the task as successful.

Exact flags are in [CLI](../reference/cli.md).

## Use Interactive Mode

From the workspace, start the full-screen terminal client:

```bash
kuku
```

Enter a prompt and press Enter to send it. If the Agent is busy, it accepts the follow-up at its next safe request boundary, or starts a new Turn if the current Turn settles first. Alt+Enter queues your message for the following Turn. Shift+Enter or Ctrl+J inserts a new line.

Use `/new` to start a draft for a fresh Session or `/resume` to reopen a Session in the current workspace. Use `/agents` to choose the next recipient. Press Esc in the conversation to interrupt the selected execution and keep your draft. In a menu, Esc closes the menu. A terminal window can disconnect while the server continues its work. [Terminal commands](../reference/cli.md#terminal-interface)

## Continue Existing Work

Resume a known session:

```bash
kuku run --session <session-id> "continue"
```

Or continue the most recent session:

```bash
kuku run --continue "continue"
```

## Inspect Output

- `kuku show <session-id>` for the final answer
- `kuku events <session-id>` for the persisted event log
- `kuku list` for recent sessions in the current workspace

## Adjust Output Mode

Use these when another tool or script will consume the result:

- `--json` for one final JSON line with full metrics
- `--stream-json` for realtime JSON lines (final event includes metrics)
- `--verbose` for human-readable output with durable state, settings, and usage
- `--raw` for plain text

### JSON output structure

`--json` returns the durable result for the selected Agent. `--stream-json` includes transient progress and the same final object. The CLI reference owns the [output fields and request controls](../reference/cli.md#kuku-run). Usage is provider-reported; an unavailable value is not proof of zero usage.

A durable question or authorization returns `event: "waiting"` with its exact request identity. Reply using `kuku reply <session-id> <request-id> --answer "answer"`, `--allow`, or `--deny`. Preserve the same workspace and data directory. See [Manage Sessions](manage-sessions.md).

Ctrl-C waits for owned command cleanup. The Session log and retained output survive closing and reopening.

For storage and event semantics, see [File Layout](../reference/file-layout.md) and [Events](../reference/events.md).
