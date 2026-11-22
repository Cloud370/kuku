# Configuration

## Config file location

By default, kuku reads `~/.kuku/config.toml`, or `$KUKU_HOME/config.toml` when `KUKU_HOME` is set.

Pass `--config PATH` to use another file, and run `kuku server status` to check which file the local service is actually using. To switch a running managed service to a different config file, wait until it is idle and run `kuku server restart --config PATH`. [File layout](../reference/file-layout.md) · [Run the server](../guides/run-the-server.md)

## Interactive setup

When you run `kuku` with no valid config or missing credentials for the default model, the welcome screen points you to `/model` to configure your first route. You can open the same flow directly with `kuku init`. Existing configs are preserved, and malformed configs are reported rather than overwritten.

From setup, choose either:

- **Edit config file** — opens the config path without sending any provider requests.
- **Find and test models** — discovers models from the provider and runs short text, thinking, and optional image checks (4,096-token output cap, 30 s timeout per test; may use API balance).

The wizard lets you pick a preset, reuse a saved connection, or enter a custom URL, network mode (system proxy env, direct, HTTP, SOCKS5, or SOCKS5H), and API key. For a custom base URL, setup probes eligible protocols (`Responses` → `Chat Completions` → `Messages`) after you select a model, and asks you to choose if results are ambiguous. If the model catalog cannot be read, you can still type a known model ID directly.

Use the model picker to search and select one or more models (`Space` to multi-select, `Enter` to confirm), then review test outcomes in **Model tests** or adjust thinking, output limits, capabilities, and pricing under **Settings**. Run **Settings → Test tools** to verify synthetic tool calls on the route — a working text reply alone does not confirm tool support. If a Chat route requires Responses for tools, switch under **Settings → API type → Responses** and re-test before saving.

Inside an active Session, run `/model` (or press `Ctrl+L`) to add or edit connections, switch the current Agent's route (**Use in this Session**), or save a new default (**Set as default**). [Terminal controls](../reference/cli.md#slash-commands) · [Config reference](../reference/config.md)

## Providers and credentials

The default config defines two providers:

- `provider.anthropic` with `api_key = "$ANTHROPIC_API_KEY"`
- `provider.openai` with `api_key = "$OPENAI_API_KEY"`

Supply keys either as an environment reference (`api_key = "$VARIABLE_NAME"`) or a key file (`api_key = "file:<absolute-path>"`). When you paste a key into interactive setup, kuku writes it to an owner-only plaintext file under `$KUKU_HOME/credentials/` and puts only the `file:` reference in `config.toml`. These files are not encrypted. [Config reference](../reference/config.md) · [File layout](../reference/file-layout.md)

Environment variables are resolved by the server process, not the terminal client. If you export a new key in your shell after the local service is already running, run `kuku server restart` once it is idle. For remote servers, set the key on the server machine. Other string fields in `config.toml` also expand `$ENV_VAR_NAME` on load (for example, `base_url = "$KUKU_ANTHROPIC_BASE_URL"`).

### Recover an authentication failure

If a provider request fails authentication in the TUI, press `F2` → **Repair credentials**. On a managed local service, paste the replacement key and confirm **Save credentials and restart local service**. Only that provider's `api_key` is updated; the key is stored in `$KUKU_HOME/credentials/` and is not validated on save. If the service is busy with another task, you can retry the restart once idle. After restart, kuku restores your Session, Agent, and draft so you can explicitly retry the original input.

For remote or token-authenticated servers, fix the credentials on the server side and restart it there — the TUI never writes local key files for remote connections.

## Model tiers

Named tiers bundle a connection, model, thinking level, and token limits (for example `strong`, `balanced`, `light`). `default_model = "balanced"` sets the default. Override it per run with `--tier <name>`, or pick an exact route with `--connection <name> --model <model-id>`.

## Common config commands

```bash
kuku config show
kuku config validate
kuku config set model.balanced.think high
```

## Other sections

Non-provider sections in `config.toml`:

- `[discovery]` — Agent and Skill auto-discovery
- `[handoff]` — long-session summarization thresholds
- `[logs]` — observability log retention (always on; defaults to `max_age_days = 14`, `max_total_size_mb = 512`)
- `[plugin]` — package extension loading
- `[update]` — release manifest source and channel (defaults to `source = "github"`, `channel = "stable"`)

See [Config reference](../reference/config.md) for exact keys and [Update Manifest](../reference/update-manifest.md) for release channels.
