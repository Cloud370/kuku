# Environment variables

## Runtime variables

| Variable | Meaning |
|---|---|
| `KUKU_HOME` | Override the default data directory (`~/.kuku`) |
| `KUKU_TUI_STATUSLINE` | Comma-separated status-line fields for this TUI launch (`none` hides it). [Terminal preferences](config.md#terminal-preferences) |
| `KUKU_TOKEN` | Bearer token for connecting to an explicit `--remote` server |
| `KUKU_SERVER_PASSWORD` | Bearer token required by a server started via `kuku server` |
| `KUKU_PROVIDER_TRACE` | `1` enables full provider traces; `0` disables traces |

## Config references

Choose your own variable names. A complete `$NAME` value in `config.toml` explicitly references that variable. Set `base_url = "$MY_GATEWAY_URL"` in `[provider.work]` and set `model = "$MY_MODEL_ID"` in `default_model`. To apply model settings to that ID, use the quoted model key `[model.work."$MY_MODEL_ID"]`.

kuku expands URL and model references when loading the config. API keys and custom headers keep their `$NAME` references until request preparation or dispatch. Custom request body strings keep literal dollar signs. API keys can also use `file:<absolute-path>`. [Config reference](config.md)

Set the referenced variables in the process that loads the config. For provider requests from CLI or TUI, this is the **server**. Local `kuku config` commands read the current invocation's environment. If the running service was started before you set the variables, restart it when idle. [Configuration guide](../start/configuration.md)

## Provider trace logs

Prerelease and untagged development builds record lightweight protocol diagnostics by default; stable releases default to off. Diagnostic JSONL records capture Session/Turn/request correlation, model and route, HTTP status, SSE sequence and terminal markers, failure kind, and anonymized reasoning-entry metadata (omitting request/response bodies, headers, and opaque payloads).

Setting `KUKU_PROVIDER_TRACE=1` captures request bodies, request and response headers, and raw SSE frames. Credential headers and inline image data are redacted from requests, but prompts, tool results, and replies can still appear in the trace. HTTP error response bodies are not written to the trace. For requests containing inline images, provider error details are also omitted from the session's error message and raw error frames are not traced. SDK embedders can configure the trace policy via `SessionOptions::provider_trace` or `Query::provider_trace`.

Traces write to rotating JSONL segments under `$KUKU_HOME/logs/provider-trace/`. They obey [log retention](config.md#logs) and are capped at write time to `min(64 MiB, total log budget)`, with each segment capped at `min(8 MiB, trace budget)` (oldest segments evicted first).

## Hook process variables

When kuku runs a package hook, it injects:

| Variable | Meaning |
|---|---|
| `KUKU_SESSION_DIR` | Absolute path to the current Session directory |
| `KUKU_WORKSPACE` | Absolute path to the current workspace |
| `KUKU_PACKAGE_DIR` | Absolute path to the package root |

Hook processes also inherit `PATH`, `HOME`, `USERPROFILE` (Windows), `LANG`, and `LC_ALL`. Other environment variables and secrets are not forwarded automatically.
