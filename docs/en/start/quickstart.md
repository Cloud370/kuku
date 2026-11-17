# Quickstart

## Configure a model

Open a terminal in the directory where the Agent should work. If you are setting up a new kuku home, run `kuku init` once. Existing installations can reuse their `config.toml`.

The generated `balanced` tier uses the `anthropic` connection. Set `ANTHROPIC_API_KEY` in the shell before starting the local service. To use OpenAI as your default, set `OPENAI_API_KEY`, select the `openai` connection in the default model tier, and enter an exact model ID available to your account. [Configuration](configuration.md)

## Start a task in the terminal

```bash
kuku
```

Enter `Inspect the files in this directory and tell me how to run its tests` and press Enter. The first send creates the Session. The status line identifies the workspace, Agent and model. Read the response and inspect tool results before making a follow-up request.

While the Agent works, Enter sends a follow-up at the next safe model-request boundary. Use Alt+Enter to queue it for the next Turn. Closing the terminal client leaves accepted work running. [Run a task](../guides/run-a-task.md)

## Return to the same Session

In another terminal, change to the same workspace, launch `kuku`, and use `/resume` to select the Session. The browser interface can also open the Session on the same service when you use a build with embedded Web assets. [Run the server](../guides/run-the-server.md)

## Run from a script

```bash
kuku run --json "Inspect the files in this directory"
kuku list
kuku show SESSION_ID
```

Replace `SESSION_ID` with the printed ID. For exact endpoint selection, pass `--connection openai --model YOUR_MODEL_ID` before the prompt. [CLI reference](../reference/cli.md)
