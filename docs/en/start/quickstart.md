# Quickstart

## Configure a model

Open a terminal in your project directory. On a fresh `KUKU_HOME`, run `kuku init` once (existing setups can reuse their `config.toml`).

The default `balanced` tier points to the `anthropic` connection, so export `ANTHROPIC_API_KEY` before starting the local service. To default to OpenAI instead, export `OPENAI_API_KEY`, point the default tier at the `openai` connection, and set a model ID available on your account. See [Configuration](configuration.md).

## Start a task in the TUI

```bash
kuku
```

Type `Inspect the files in this directory and tell me how to run its tests` and press Enter. Your first message creates a Session. The status line shows the workspace, Agent, and active model.

While the Agent is running:

- **Enter** sends a follow-up that is picked up at the next safe model-request boundary.
- **Alt+Enter** queues it for the next Turn.
- Closing the terminal leaves accepted work running in the local service.

See [Run a task](../guides/run-a-task.md) for more workflow tips.

## Return to the same Session

From another terminal in the same workspace, start `kuku` and run `/resume` to pick up the Session. If your build embeds the Web UI, `kuku web` opens the same service in a browser. [Run the server](../guides/run-the-server.md)

## Run from a script

```bash
kuku run --json "Inspect the files in this directory"
kuku list
kuku show SESSION_ID
```

Replace `SESSION_ID` with the ID printed by `kuku run`. To pick an exact route on the fly, pass `--connection openai --model YOUR_MODEL_ID` before the prompt. [CLI reference](../reference/cli.md)
