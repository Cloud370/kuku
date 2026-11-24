# Configuration

## Config file location

By default, kuku reads `~/.kuku/config.toml`, or `$KUKU_HOME/config.toml` when `KUKU_HOME` is set.

Pass `--config PATH` to use another file, and run `kuku server status` to check which file the local service is actually using. To switch a running managed service to a different config file, wait until it is idle and run `kuku server restart --config PATH`. [File layout](../reference/file-layout.md) · [Run the server](../guides/run-the-server.md)

## Interactive setup

When you run `kuku` with no valid config or missing credentials for the default model, the welcome screen points you to `/model` to configure your first route. You can open the same flow directly with `kuku init`. Existing configs are preserved, and malformed configs are reported rather than overwritten.

From setup, choose either:

- **Edit config file** — opens the config path without sending any provider requests.
- **Find and test models** — discovers models from the provider and runs short text, thinking, and optional image checks (4,096-token output cap, 30 s timeout per test; may use API balance).

ChatGPT setup reads your account catalog. You can opt in to generation tests from **Settings**.

Use the model picker to search and select one or more models (`Space` to multi-select, `Enter` to confirm), then review test outcomes in **Model tests** or adjust thinking, output limits, capabilities, and pricing under **Settings**. Run **Settings → Test tools** to verify synthetic tool calls on the route — a working text reply alone does not confirm tool support. If a Chat route requires Responses for tools, switch under **Settings → API type → Responses** and re-test before saving.

Choose a custom connection, saved connection, or preset. For API-key connections, enter the provider URL, network path, and key. A custom connection takes its name from the URL host and any non-default port. Connections with the same host and port get distinct names. Setup uses the protocol from a saved connection, preset, or standard request URL. After model selection, setup tests the eligible protocols for a new custom base URL in parallel. It prefers Responses, then Chat Completions, then Messages when the preceding protocols are unsupported. An ambiguous result opens the API type picker. The network choices are the proxy environment, a direct connection, or an HTTP, SOCKS5, or SOCKS5H proxy. When a bare address has more than one standard candidate path, the editor checks them concurrently and opens the first usable model list automatically. The screen lists each request and its outcome. An unreadable catalog still allows a known model ID to be entered directly.

Inside an active Session, run `/model` (or press `Ctrl+L`) to add or edit connections, switch the current Agent's route (**Use in this Session**), or save a new default (**Set as default**). [Terminal controls](../reference/cli.md#slash-commands) · [Config reference](../reference/config.md)

When saving a model with no configured thinking level, setup selects `medium` when supported. Otherwise it uses the lower middle of known ordered levels, the only available level, or a valid provider-declared default when the levels have no known order. A route with insufficient capability information follows the provider default. Manual settings take precedence.

To edit service tiers you added, select the route, press `e`, and open **Settings → Model capabilities → Custom service tiers**. [Model and service tier keyboard controls](../reference/cli.md#slash-commands) · [Config reference](../reference/config.md)

## Providers and credentials

The default config defines two providers:

- `provider.anthropic` with `api_key = "$ANTHROPIC_API_KEY"`
- `provider.openai` with `api_key = "$OPENAI_API_KEY"`

Supply keys either as an environment reference (`api_key = "$VARIABLE_NAME"`) or a key file (`api_key = "file:<absolute-path>"`). When you paste a key into interactive setup, kuku writes it to an owner-only plaintext file under `$KUKU_HOME/credentials/` and puts only the `file:` reference in `config.toml`. These files are not encrypted. [Config reference](../reference/config.md) · [File layout](../reference/file-layout.md)

Environment variables are resolved by the server process, not the terminal client. If you export a new key in your shell after the local service is already running, run `kuku server restart` once it is idle. For remote servers, set the key on the server machine. Other string fields in `config.toml` also expand `$ENV_VAR_NAME` on load (for example, `base_url = "$KUKU_ANTHROPIC_BASE_URL"`).

### Sign in to a subscription on a headless host

In `kuku init` or **Add model** in `/model`, choose **Preset · ChatGPT subscription**. Choose the network mode and optionally name the account. Choose **Open authorization link** in a terminal with hyperlinks, or use **Ctrl+Y** to copy the complete address between the URL separators. After authorizing, paste the callback URL from its address bar into kuku. If the browser cannot reach the callback page, copy the address from the error page. The callback prompt shows a cursor while keeping the pasted URL hidden. If OpenAI rejects the code, use the fresh authorization link shown by kuku. After sign-in, the account is saved and setup starts finding models for that account.

From a shell you can also run `kuku auth login chatgpt [name]` on the host running kuku. To reauthorize a saved account, use its saved name. Omit the name to derive one from the authorized identity.

Setup defaults to the environment's proxy settings, with options for direct access or an explicit proxy. The selected mode applies to token exchange, renewal, model discovery, and inference. An explicit proxy failure stops the request. Set a SOCKS proxy to `socks5h://` to resolve provider addresses at the proxy. The browser follows its own network settings.

Adding or editing a ChatGPT model refreshes the model list with the selected account's access token. Setup shows the account name and masked email when available. Give registrations distinct names if you use multiple workspaces with the same email. OpenAI's public sign-in API does not specify a field for the plan name, such as Plus or Pro.

In the model editor, **Settings → Service tier** uses the speeds listed by OpenAI for the selected account and model. Speeds without a catalog declaration are marked unconfirmed. Choose **Settings → Test model** to opt into a text trial with the current model, speed, and thinking settings. The trial uses your ChatGPT plan. **Model tests** shows the speed reported for a completed trial, when available, or the error returned by OpenAI.

For ChatGPT, open [Manage usage](https://chatgpt.com/settings/usage) to review plan and app limits. kuku can report a limit error from ChatGPT and the token counts returned by completed requests. Other subscription providers can expose remaining allowance when their integration supplies it.

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
