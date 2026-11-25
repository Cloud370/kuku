# Configuration

## Config file location

Run `kuku config show` to see the selected file with credentials redacted. Run `kuku config validate` for its absolute path, diagnostics, and whether the local service has loaded that version. The [CLI reference](../reference/cli.md#kuku-config) explains how kuku selects the file and how to target another with `--config PATH`.

Check the running service's config path with `kuku server status`. To switch a managed service to a different file, wait until it is idle and run `kuku server restart --config PATH`. [File layout](../reference/file-layout.md) · [Run the server](../guides/run-the-server.md)

## Interactive setup

Run `kuku` from your project directory. When model setup is required on startup, kuku opens the setup page automatically. If you exit setup, press `Ctrl+L` or use `/model` to reopen it. Press `F1` during setup to open the configuration guide for your build. If kuku reports a malformed config, correct the indicated file or use another file with `--config PATH`.

From setup, choose either:

- **Edit config file** — opens the config path without sending any provider requests.
- **Find and test models** — discovers models from the provider and runs short text, thinking, and optional image checks (4,096-token output cap, 30 s timeout per test; may use API balance).

ChatGPT setup reads your account catalog. You can opt in to generation tests from **Settings**.

Use the model picker to search and save one exact model in the initial setup. Review test outcomes in **Model tests** or adjust thinking, output limits, capabilities, and pricing under **Settings**. Run **Settings → Test tools** to verify synthetic tool calls on the route. If a Chat route requires Responses for tools, switch under **Settings → API type → Responses** and re-test before saving.

Choose a custom connection, saved connection, or preset. For API-key connections, enter the provider URL, network path, and key. A custom connection takes its name from the URL host and any non-default port. Connections with the same host and port get distinct names. Setup uses the protocol from a saved connection, preset, or standard request URL. After model selection, setup tests the eligible protocols for a new custom base URL in parallel. It prefers Responses, then Chat Completions, then Messages when the preceding protocols are unsupported. An ambiguous result opens the API type picker. The network choices are the proxy environment, a direct connection, or an HTTP, SOCKS5, or SOCKS5H proxy. When a bare address has more than one standard candidate path, the editor checks them concurrently and opens the first usable model list automatically. The screen lists each request and its outcome. An unreadable catalog still allows a known model ID to be entered directly.

To add a model later, open `/model` and choose **Add model or connection**. Select an existing model and press `e` to edit it.

## Select a model

In `/model`, highlight a connection and model and press Enter to switch the current Agent. To change the default for new Sessions, highlight a model and press `d`, then confirm the config change. These actions let you change the current Agent and the default independently. [Terminal controls](../reference/cli.md#slash-commands) · [Config reference](../reference/config.md)

When saving a model with no configured thinking level, setup selects `medium` when supported. Otherwise it uses the lower middle of known ordered levels, the only available level, or a valid provider-declared default when the levels have no known order. A route with insufficient capability information follows the provider default. Manual settings take precedence.

To edit service tiers you added, select the route, press `e`, and open **Settings → Model capabilities → Custom service tiers**. [Model and service tier keyboard controls](../reference/cli.md#slash-commands) · [Config reference](../reference/config.md)

## Providers and credentials

Choose whether to enter a key or reference an environment variable in setup. Enter the variable name you set for that service, for example `MY_GATEWAY_KEY`. The config records it as `api_key = "$MY_GATEWAY_KEY"`. When you paste a key, kuku writes it to an owner-only plaintext file under the data directory's `credentials/` and records its `file:` reference. [Environment variables](../reference/environment-variables.md) · [File layout](../reference/file-layout.md)

To write the file by hand, start with the [one-model TOML](../reference/config.md#minimal-config). Set referenced variables on the machine that runs the service. After you change the environment of an already-running local service, start a replacement with `kuku server restart` once the service is idle.

### Sign in to a subscription on a headless host

In first-run setup or `/model`, choose **Preset · ChatGPT subscription** when adding a connection. Choose the network mode and optionally name the account. Choose **Open authorization link** in a terminal with hyperlinks, or use **Ctrl+Y** to copy the complete address between the URL separators. After authorizing, paste the callback URL from its address bar into kuku. If the browser cannot reach the callback page, copy the address from the error page. The callback prompt shows a cursor while keeping the pasted URL hidden. If OpenAI rejects the code, use the fresh authorization link shown by kuku. After sign-in, the account is saved and setup starts finding models for that account.

From a shell you can also run `kuku auth login chatgpt [name]` on the host running kuku. Sign-in without a name saves a short connection name such as `chatgpt` or `chatgpt-2`. To reauthorize an account, use its saved connection name or account alias.

Setup defaults to the environment's proxy settings, with options for direct access or an explicit proxy. The selected mode applies to token exchange, renewal, model discovery, and inference. An explicit proxy failure stops the request. Set a SOCKS proxy to `socks5h://` to resolve provider addresses at the proxy. The browser follows its own network settings.

Adding or editing a ChatGPT model refreshes the model list with the selected account's access token. Setup shows the account name and masked email when available. Give registrations distinct names if you use multiple workspaces with the same email. OpenAI's public sign-in API does not specify a field for the plan name, such as Plus or Pro.

In the model editor, **Settings → Service tier** uses the speeds listed by OpenAI for the selected account and model. Speeds without a catalog declaration are marked unconfirmed. Choose **Settings → Test model** to opt into a text trial with the current model, speed, and thinking settings. The trial uses your ChatGPT plan. **Model tests** shows the speed reported for a completed trial, when available, or the error returned by OpenAI.

For ChatGPT, open [Manage usage](https://chatgpt.com/settings/usage) to review plan and app limits. kuku can report a limit error from ChatGPT and the token counts returned by completed requests. Other subscription providers can expose remaining allowance when their integration supplies it.

### Recover an authentication failure

If a provider request fails authentication in the TUI, press `F2` → **Repair credentials**. On a managed local service, paste the replacement key and confirm **Save credentials and restart local service**. Only that provider's `api_key` is updated; the key is stored in `$KUKU_HOME/credentials/` and is not validated on save. If the service is busy with another task, you can retry the restart once idle. After restart, kuku restores your Session, Agent, and draft so you can explicitly retry the original input.

For remote or token-authenticated servers, fix the credentials on the server side and restart it there — the TUI never writes local key files for remote connections.

## Common config commands

```bash
kuku config show
kuku config validate
kuku config models
kuku config models --discover --connection work
kuku config test --connection work --model MODEL_ID
```

Replace `work` with a connection saved in your config. To create or edit a config from a script, see [Run a task](../guides/run-a-task.md).

## Other sections

Non-provider sections in `config.toml`:

- `[discovery]` — Agent and Skill auto-discovery
- `[handoff]` — long-session summarization thresholds
- `[logs]` — observability log retention (always on; defaults to `max_age_days = 14`, `max_total_size_mb = 512`)
- `[plugin]` — package extension loading
- `[update]` — release manifest source and channel (defaults to `source = "github"`, `channel = "stable"`)

See [Config reference](../reference/config.md) for exact keys and [Update Manifest](../reference/update-manifest.md) for release channels.
