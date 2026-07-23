# Quickstart

## 1. Start kuku

```bash
kuku
```

Open the credential URL printed in the terminal. On first run, the Web UI guides you through provider and workspace setup.

## 2. Configure a Provider

Enter a provider API key in the Web setup. For terminal-only setup, the default config also accepts `ANTHROPIC_API_KEY` or `OPENAI_API_KEY`:

```bash
export ANTHROPIC_API_KEY="..."
```

See [Environment Variables](../reference/environment-variables.md) and [Config](../reference/config.md).

## 3. Run a First Task

Create and run the task in the Web UI.

To use the terminal instead, run `kuku run say hello` after setup.

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
