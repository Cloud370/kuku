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

Choose **Find and test models** once to allow model discovery and short text or image tests during setup. Each test request may use your API balance and has an output limit of 4096 tokens and a timeout of 30 seconds. **Edit config file** opens the config path with zero provider requests.

Setup checks which thinking values each selected model accepts, starting with the values declared by the API. After setup knows the API type, text, thinking, and image checks can overlap. Generation tests share two slots per selected model, up to six slots. The setup screen shows overall progress across the selected models. Completed requests advance it while other requests are still running. The overview shows how many requests have finished and how long active checks have been running. When a check has an error or an unverified image result, the review screen focuses **Model tests**. The compact table gives each model separate Text, Thinking, and Image columns with live results and retry times, even in wide terminals. **Ready** means the model returned text. Open **Fix failed models** to retry a failed text check, skip that selection, or change the key or URL when the error points there. If another model returned text, **Use another default** lets you switch to it. **Model tests** contains individual results, including optional image and thinking checks. While tests run, Enter opens **View requests** by default. **Save early** asks for confirmation before it keeps completed results and stops pending requests. Going back from active tests also asks before stopping them. The footer shows what Esc does on each screen.

Choose a custom connection, saved connection, or preset. Enter the provider URL, network path, and key. A custom connection takes its name from the URL host and any non-default port. Connections with the same host and port get distinct names. Setup uses the protocol from a saved connection, preset, or standard request URL. After model selection, setup tests the eligible protocols for a new custom base URL in parallel. It prefers Responses, then Chat Completions, then Messages when the preceding protocols are unsupported. An ambiguous result opens the API type picker. The network choices are the proxy environment, a direct connection, or an HTTP, SOCKS5, or SOCKS5H proxy. When a bare address has more than one standard candidate path, the editor checks them concurrently and opens the first usable model list automatically. The screen lists each request and its outcome. An unreadable catalog still allows a known model ID to be entered directly.

Models stay in the order returned by the provider, including filtered search results. Type to search, use the arrows to move, and press Enter to select one. Space marks models for a multi-selection, and Enter accepts the marked set. Choose a default when creating a configuration with several models. Selecting models starts tests in the background. The review screen shows the request address, model progress, and test status. Open **View requests** during testing or **Model tests** afterward to see individual requests. Open **Settings** to change thinking, output, model capabilities, or costs. Set optional token rates from **Costs** and add **Request cost** to the terminal status line to see monetary totals. [Proxy and pricing configuration](../reference/config.md#providername)

Choose **Settings → Test tools** to check a synthetic tool call and its reply on the selected route. A working text reply alone does not establish tool support. On a text-only Chat route that requires Responses for tools, choose **Settings → API type → Responses** to test the new route before saving.

The Turn summary highlights cache reads below 50% after the first two comparable Turns on the current route. Two consecutive low Turns raise a yellow status-line notice. Open it with the mouse or F2 and select **Dismiss**. A new route or context starts a new baseline.

In a Session, open `/model` or press Ctrl+L. **Add model or connection** and **Edit** open the shared form. An existing connection can be reused with its saved credential. **Use in this Session** changes the selected Agent. **Set as default** saves the route for new Sessions. [Terminal thinking controls](../reference/cli.md#slash-commands) · [Config reference](../reference/config.md)

## Provider Setup

The default config defines two providers:

- `provider.anthropic` with `api_key = "$ANTHROPIC_API_KEY"`
- `provider.openai` with `api_key = "$OPENAI_API_KEY"`

Use an explicit `api_key = "$VARIABLE_NAME"` reference or `api_key = "file:<absolute-path>"`. The interactive settings flow stores new pasted keys under the service data directory and writes only the absolute file reference to TOML. These files are plaintext, not encrypted. See [Config](../reference/config.md) and [File layout](../reference/file-layout.md).

When your login directory contains the key store, choose **Use a trial workspace** to paste a key and open kuku in the data directory's `trial-workspace/` subdirectory. The key store stays outside that workspace. You can continue with the Environment source or exit and run `kuku --workspace /path/to/project` for a project you already have. To return to the trial workspace later, pass its path to `--workspace`.

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
