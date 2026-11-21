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

When you start interactive `kuku` without a local config file, with an empty file, or its existing config has a missing, unreadable, or empty credential for the effective default tier's provider, the TUI offers shared settings before opening the session. It preserves and pre-fills an existing config, and checks only that default-route provider using the normal environment, file, or plaintext credential resolver; it does not test authentication. A readable, non-empty existing default credential skips setup, and malformed config is reported rather than overwritten. The same settings flow is available as `kuku init` (or `kuku-cli init`) for local connections; it requires a TTY and makes no writes when run without one. It honors `--data-dir`, `KUKU_HOME`, and `--config`. For `kuku init` with a live managed service, setup uses its authenticated active config path; a conflicting explicit `--config` is an error. First-run onboarding is not opened for a live service, a service whose status cannot be checked, or a remote connection; it never starts or restarts a service just to configure it.

Choose **Find and test models** once to allow model discovery and short text or image tests during setup. Each test request may use your API balance and has an output limit of 4096 tokens and a timeout of 60 seconds. **Edit config file** opens the config path with zero provider requests.

Setup checks which thinking values each selected model accepts, starting with the values declared by the API. The save screen shows each selected model's progress, including queued checks, retry countdowns, and usable thinking values. **Model tests** groups all checks and their results by model. Generation tests share two to four slots depending on how many models are selected. You can save completed results while other tests are pending. An accepted value can be sent in normal use. If the API reports an applied value or thinking activity, the test shows it separately. The footer shows what Esc does on each screen.

Choose a custom connection, saved connection, or preset. Enter the provider URL, network path, and key. Setup uses the protocol from a saved connection, preset, or standard request URL. A new custom base URL is tested after model selection. An ambiguous result opens the API type picker. The network choices are the proxy environment, a direct connection, or an HTTP, SOCKS5, or SOCKS5H proxy. When a bare address has more than one standard candidate path, the editor checks them concurrently and opens the first usable model list automatically. The screen lists each request and its outcome. An unreadable catalog still allows a known model ID to be entered directly.

Models stay in the order returned by the provider, including filtered search results. Type to search, use the arrows to move, and press Enter to select one. Space marks models for a multi-selection, and Enter accepts the marked set. Choose a default when creating a configuration with several models. Selecting models opens the save screen and starts tests in the background. Watch progress there or open **Model tests** for individual requests. Saving stops outstanding tests, so you can start using the configuration immediately. The save screen shows the model, request address, and test status. Open **Settings** to change thinking, output, model capabilities, or costs. Set optional token rates from **Costs** and add **Request cost** to the terminal status line to see monetary totals. [Proxy and pricing configuration](../reference/config.md#providername)

In a Session, open `/model` or press Ctrl+L. **Add model or connection** and **Edit** open the shared form. An existing connection can be reused with its saved credential. **Use in this Session** changes the selected Agent. **Set as default** saves the route for new Sessions. `/thinking` offers a separate Session override and persistent model setting. [Config reference](../reference/config.md)

## Provider Setup

The default config defines two providers:

- `provider.anthropic` with `api_key = "$ANTHROPIC_API_KEY"`
- `provider.openai` with `api_key = "$OPENAI_API_KEY"`

Use an explicit `api_key = "$VARIABLE_NAME"` reference or `api_key = "file:<absolute-path>"`. The interactive settings flow stores new pasted keys under the service data directory and writes only the absolute file reference to TOML. These files are plaintext, not encrypted. See [Config](../reference/config.md) and [File layout](../reference/file-layout.md).

Set the environment for the server process. If you change a key in your terminal after the managed service starts, run `kuku server restart` from that terminal when the service is idle. For a remote connection, configure the key on the server machine. After validating a change to the active config file, the service uses the new configuration when it opens Sessions.

For other string settings, you can also use `$ENV_VAR_NAME` in `config.toml`. For example, `base_url = "$KUKU_ANTHROPIC_BASE_URL"` resolves during config load.

### Recover an authentication failure

After a provider authentication failure in the terminal, choose **Repair credentials** with F2, then Enter. The repair is bound to that failed connection and model. On a managed local service, enter the replacement key in the masked editor and press Enter to review it. Confirm **Save credentials and restart local service** to apply the change. Only that provider's `api_key` changes; the current model choice and other configuration stay in place. The key uses the same private file storage as setup and is never copied to the composer or logs. Saving does not validate the key.

If another execution keeps the service busy, **Credentials saved; service not restarted** lets you retry the safe restart later. The terminal distinguishes saved credentials from credentials applied to the current Session; it does not retry the failed request. After restart is confirmed for the same config and a new service instance, the terminal restores the same Session, Agent and draft. Use Continue or the shown recovery action to retry the retained original input explicitly, without submitting it again.

For remote or token-authenticated connections, repair credentials on the server and restart it safely when required, then reconnect and continue. The terminal does not write local credentials for those connections.

## Model Tiers

Optional named route examples:

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
