# Run a task

## Set up from a shell

[Install kuku](../start/install.md) and open a shell in your project directory. If a model already works in the TUI, use the same data directory and skip to [Submit a task](#submit-a-task).

For a new script setup, set a key in `MY_GATEWAY_KEY` and run the following. Use the base URL and exact model ID for your connection. The quoted heredoc leaves the variable reference intact in the config.

```bash
export KUKU_HOME="$HOME/.kuku-script"

kuku config set --patch "$(cat <<'TOML'
default_model = { connection = "work", model = "your-model-id" }

[provider.work]
format = "openai-responses"
base_url = "https://gateway.example/v1"
api_key = "$MY_GATEWAY_KEY"
TOML
)"
kuku config validate
```

Export the key before sending the first task so the managed local service inherits it. A new data directory reports `status: "valid"` and `service.state: "stopped"` until the first run starts the service. For the selected path, a running service, or model discovery, see [CLI config commands](../reference/cli.md#kuku-config).

## Submit a task

Save a prompt in `task.txt` and run it from the current directory.

```bash
cat > task.txt <<'EOF'
Inspect the project files.
Tell me how to run the tests.
EOF
kuku run --json --prompt-file task.txt > result.json
jq -r '.response' result.json
session_id="$(jq -er '.session_id' result.json)"
```

To read from a pipeline and get live JSON Lines, run `cat task.txt | kuku run --stream-json --prompt-file -`. A short task can use a positional prompt such as `kuku run "Check this project"`. Use `--workspace PATH` for a different directory, or `--connection work --model your-model-id` to select an exact pair. [CLI run options and exit codes](../reference/cli.md#kuku-run)

## Handle the result

Check the exit status of `kuku run` when scripting. A completed result contains the final `response`. For `waiting` (exit 3), get the exact request ID from `question.id` or `permission.id` and use `kuku reply` in the same workspace and data directory.

```bash
request_id="$(jq -er '.question.id' result.json)"
kuku reply "$session_id" "$request_id" --answer "Use the project test configuration"
```

For a permission request, use `.permission.id` and choose `--allow` or `--deny`. Use `--agent` if the result names an Agent other than `main`. For `failed` (exit 1), read the result's `error` and stderr, then inspect the Session with [Debug CLI runs](debug-cli.md). `Ctrl+C` requests cancellation of the active run and waits for owned commands to clean up.

## Continue existing work

```bash
kuku run --session "$session_id" --json "Now summarize what you found"
kuku show "$session_id"
```

Keep using the same workspace and data directory. Use `kuku list` to find a Session or `kuku events "$session_id"` to read its durable events. In the TUI, open `/resume` to find the same Session. [Manage Sessions](manage-sessions.md)

## Run interactively

Run `kuku` and press Enter to send a task. While the Agent is running, Enter sends a follow-up at the next safe boundary and Alt+Enter queues it for the next Turn. Shift+Enter or Ctrl+J inserts a newline and Esc interrupts while keeping your draft. Use `/new` for a fresh Session and `/agents` to pick the recipient. [Terminal controls](../reference/cli.md#terminal-interface)
