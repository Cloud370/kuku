# Quickstart

## Initialize the CLI

From the workspace where the Agent should work, run this once for a new kuku home.

```bash
kuku init
```

An existing installation can use its current `config.toml`. The CLI reads it from `KUKU_HOME` or, by default, `~/.kuku`.

## Run a first task

The generated `balanced` tier uses the `anthropic` connection. Set `ANTHROPIC_API_KEY` in the calling shell and submit a task.

```bash
kuku run "Inspect the files in this directory"
```

For OpenAI, set `OPENAI_API_KEY` in that shell. Replace `YOUR_MODEL_ID` with a model available on your account and select the configured Responses connection explicitly.

```bash
kuku run --connection openai --model YOUR_MODEL_ID "Inspect the files in this directory"
```

For repeated use of that route, point the default tier at the same connection and model in your config. [Configuration](configuration.md)

## Continue or inspect the Session

```bash
kuku list
kuku run --session SESSION_ID "Summarize what you found"
kuku show SESSION_ID
```

Replace `SESSION_ID` with the printed ID and run in the same workspace. Bare `kuku` starts a line-based prompt. It reuses the Session for subsequent inputs and accepts `exit` or `quit` between Turns. To leave work running after a client disconnects, start it through the HTTP server. [Run a task](../guides/run-a-task.md) · [Run the server](../guides/run-the-server.md)
