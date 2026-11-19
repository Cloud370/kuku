# Configuration

## Config File Location

By default, kuku uses:

```text
~/.kuku/config.toml
```

If `KUKU_HOME` is set, the service looks for:

```text
$KUKU_HOME/config.toml
```

The local service uses the selected data directory's `config.toml` by default. Choose another file with `--config`. Check the active path with `kuku server status`. To change the configuration path for an already running managed service, use `kuku server restart --config PATH` when the service is idle. [File layout](../reference/file-layout.md) · [Run the server](../guides/run-the-server.md)

## First-run TUI setup

When you start interactive `kuku` without a local config file, the TUI offers a short setup before opening the session. It uses the same resolved path as the local service, honoring `--data-dir`, `KUKU_HOME`, and `--config`, and shows both that path and the workspace. Setup is skipped when the selected config path already exists (including a malformed file) and for remote connections.

Choose a provider, a literal model ID, and a credential environment-variable **name**. Model IDs cannot start with `$`, which the config loader interprets as an environment-variable reference. The setup never displays or stores key values and makes no model request. It only checks whether the named variable has a non-empty value; it does not validate the credential or the model ID. If the variable is missing or empty, you can save and exit, set it in the shell that launches `kuku`, and restart. Canceling does not create a config.

The generated config uses `balanced` as its default tier and initializes `strong`, `balanced`, and `light` with the selected provider and model. Edit `config.toml` or use `kuku config set` to change persistent defaults and individual tiers. In the TUI, `/model` changes the selected Agent's session configuration only; it does not change the saved config default.

## Provider Setup

The default config defines two providers:

- `provider.anthropic` with `api_key = "$ANTHROPIC_API_KEY"`
- `provider.openai` with `api_key = "$OPENAI_API_KEY"`

You can keep secrets in environment variables or store a literal `api_key` in `config.toml`.

Set the environment for the server process. If you change a key in your terminal after the managed service starts, run `kuku server restart` from that terminal when the service is idle. For a remote connection, configure the key on the server machine. After validating a change to the active config file, the service uses the new configuration when it opens Sessions.

For other string settings, you can also use `$ENV_VAR_NAME` in `config.toml`. For example, `base_url = "$KUKU_ANTHROPIC_BASE_URL"` resolves during config load.

## Model Tiers

The default install defines three tiers:

- `strong`
- `balanced`
- `light`

Each tier maps to one provider, one model, a thinking level, and token limits. `default_model = "balanced"` selects the default tier. Use `--tier <name>` to choose another tier, or `--connection <name> --model <model-id>` to select an exact connection and model.

## Common Changes

Show the current config:

```bash
kuku config show
```

Validate the file:

```bash
kuku config validate
```

Set one value:

```bash
kuku config set model.balanced.think high
```

## Discovery, Handoff, Logs, Plugins, and Updates

The main non-provider sections are:

- `[discovery]` for agent and skill auto-discovery
- `[handoff]` for long-session summarization thresholds
- `[logs]` for observability log retention
- `[plugin]` for package extension loading
- `[update]` for release source and channel settings

Logs are default-on with no disable switch. The default retention is `max_age_days = 14` and `max_total_size_mb = 512`.

The exact keys live in [Config](../reference/config.md).

## Update Channels

The default update section uses:

```toml
[update]
source = "github"
channel = "stable"
```

This controls which release manifest kuku should follow. See [Update Manifest](../reference/update-manifest.md).
