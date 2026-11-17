# Quickstart

## 1. Initialize kuku

```bash
kuku init
```

This creates the default runtime directories and a starter `config.toml`.

## 2. Set a Provider API Key

The default config expects one of these environment variables:

```bash
export ANTHROPIC_API_KEY="..."
```

or:

```bash
export OPENAI_API_KEY="..."
```

See [Environment Variables](../reference/environment-variables.md) and [Config](../reference/config.md).

## 3. Run a First Task

```bash
kuku run "inspect the files in this directory"
```

Or start interactive mode:

```bash
kuku
```

No subcommand starts an interactive Session in the current workspace. The default local policy permits tools; file and command results persist under the printed Session identity.

For explicit selection, use `--tier <name>` or `--connection <name> --model <model-id>` before the prompt.

## 4. Inspect the Result

Useful follow-up commands:

```bash
kuku list
kuku show <session-id>
kuku events <session-id>
```

See [CLI](../reference/cli.md) for the full command surface.

## Next

- For a normal task flow, go to [Run a Task](../guides/run-a-task.md).
- For config details, go to [Configuration](configuration.md).
