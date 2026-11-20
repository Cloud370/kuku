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

When you start interactive `kuku` without a local config file, or its existing config has a missing, unreadable, or empty credential for the effective default tier's provider, the TUI offers shared settings before opening the session. It preserves and pre-fills an existing config, and checks only that default-route provider using the normal environment, file, or plaintext credential resolver; it does not test authentication. A readable, non-empty existing default credential skips setup, and malformed config is reported rather than overwritten. The same settings flow is available as `kuku init` (or `kuku-cli init`) for local connections; it requires a TTY and makes no writes when run without one. It honors `--data-dir`, `KUKU_HOME`, and `--config`. For `kuku init` with a live managed service, setup uses its authenticated active config path; a conflicting explicit `--config` is an error. First-run onboarding is not opened for a live service, a service whose status cannot be checked, or a remote connection; it never starts or restarts a service just to configure it.

Choose an existing named connection or a preset, then edit the current default tier's model ID. Model IDs cannot start with `$`, which the config loader interprets as an environment-variable reference. Step 3 shows the environment variable name and whether a non-empty value is detected. Detection is only a presence check, not authentication, and the value is never displayed. When the selected environment reference or preset variable is missing or empty and private storage is safe, `Paste API key (save locally)` is selected by default; a detected variable keeps Environment selected. If storage is unavailable, Environment stays selected and Review gives guidance to set the variable before launch. Existing file and plaintext credentials remain on the stored-key source. You can explicitly select Environment, and setup does not switch away from that choice later. The sources never fall back to one another.

Pasted-key input is masked. When private storage is safe, the key is saved as plaintext (not encrypted) in a private file under `<data-dir>/credentials/`, outside the workspace. If a first-run credential is not ready, Review defaults to the paste-key repair action; when storage is unavailable, it instead shows safe guidance to set the environment variable before launch. A blank paste stays on the credential screen. `Save and exit` remains an explicit secondary choice; if the default credential is still missing, first-run setup will be offered again next launch. A ready first-run credential defaults to `Save and continue`. Setup makes no model request and does not validate provider authentication.

The first setup of a new config retains the previous `strong`, `balanced`, and `light` defaults. Editing an existing config changes only its selected default tier and the chosen connection fields; comments, custom endpoints, provider options, other providers, and other tiers are preserved. Existing inline credentials are never shown and move to private storage only when you explicitly save. A file reference already under this service's credentials directory is kept when the key input is blank. Other valid private file references are moved into this data directory only when you explicitly save; the original file is preserved. Malformed configs are not overwritten. Canceling or going back does not save config or key files.

The generated config uses `balanced` as its default tier and initializes `strong`, `balanced`, and `light` with the selected provider and model. Edit `config.toml` or use `kuku config set` to change persistent defaults and individual tiers. In the TUI, ordinary `/model` and `/thinking` choices apply only to the selected Agent's Session. To persist the resolved connection and model, choose **Set current model as default** and confirm the config path and tier. This writes an ordinary tier named `user-default` only when no existing tier matches; it does not create a new priority or change the current Session. The saved default applies to new Sessions and Sessions without a separately chosen model; reopening an implicit Session can therefore use the updated default.

## Provider Setup

The default config defines two providers:

- `provider.anthropic` with `api_key = "$ANTHROPIC_API_KEY"`
- `provider.openai` with `api_key = "$OPENAI_API_KEY"`

Use an explicit `api_key = "$VARIABLE_NAME"` reference or `api_key = "file:<absolute-path>"`. The interactive settings flow stores new pasted keys under the service data directory and writes only the absolute file reference to TOML. These files are plaintext, not encrypted. See [Config](../reference/config.md) and [File layout](../reference/file-layout.md).

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
