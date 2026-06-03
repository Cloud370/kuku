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

- `--json` for one final JSON line with full metrics
- `--stream-json` for realtime JSON lines (final event includes metrics)
- `--verbose` for human-readable output with detailed usage, tools, and response
- `--raw` for plain text

### JSON output structure

`--json` and `--stream-json` emit a `session_completed` object:

```json
{"type":"session","event":"completed","session_id":"...","tier":"balanced",
 "model":"claude-sonnet-4-6","turns":1,"input_tokens":4500,"output_tokens":900,
 "cache_read_input_tokens":27000,"cache_creation_input_tokens":0,"duration_ms":7910,
 "response":"READY",
 "usage":{"total_input_tokens":31500,"total_tokens":32400,
          "cache_hit_rate":0.857,"model_requests":3,"thinking_duration_ms":3200},
 "tools":{"total_calls":5,"names":["read_file","write_file","bash"],
          "denied":0,"errors":1,"rounds":2}}
```

All fields are always present, including `cache_creation_input_tokens` (defaults to 0). `response` is the full final assistant text. On interrupt (`session_interrupted`), `response` is partial text or `null`.

## Roll Back a Turn

Inside interactive mode, use `/undo` to roll back to an earlier turn.

For session storage and event semantics, see [File Layout](../reference/file-layout.md) and [Events](../reference/events.md).
