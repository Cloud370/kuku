# Run a Task

Use this flow when kuku is already installed and configured.

## Start a New Task

For a one-shot run:

```bash
kuku run "check this project"
```

To choose a tier explicitly:

```bash
kuku run --model strong "review this diff"
```

Exact flags are in [CLI](../reference/cli.md).

## Use Interactive Mode

For back-and-forth work, start the interactive CLI:

```bash
kuku
```

Then enter prompts one turn at a time.

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

- `--json` for one final JSON line
- `--stream-json` for realtime JSON lines
- `--raw` for plain text

## Roll Back a Turn

Inside interactive mode, use `/undo` to roll back to an earlier turn.

For session storage and event semantics, see [File Layout](../reference/file-layout.md) and [Events](../reference/events.md).
