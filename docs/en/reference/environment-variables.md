# Environment variables

## Runtime variables

| Variable | Meaning |
|---|---|
| `KUKU_HOME` | Override the default data directory (`~/.kuku`) |
| `KUKU_TUI_STATUSLINE` | Comma-separated status-line fields for this TUI launch (`none` hides it). [Terminal preferences](config.md#terminal-preferences) |
| `KUKU_TOKEN` | Bearer token for connecting to an explicit `--remote` server |
| `KUKU_SERVER_PASSWORD` | Bearer token required by a server started via `kuku server` |
| `KUKU_PROVIDER_TRACE` | `0` disables provider traces; full traces are on by default (`1`) |

## Config references

Choose your own variable names. A complete `$NAME` value in `config.toml` explicitly references that variable. Set `base_url = "$MY_GATEWAY_URL"` in `[provider.work]` and set `model = "$MY_MODEL_ID"` in `default_model`. To apply model settings to that ID, use the quoted model key `[model.work."$MY_MODEL_ID"]`.

kuku expands URL and model references when loading the config. API keys and custom headers keep their `$NAME` references until request preparation or dispatch. Custom request body strings keep literal dollar signs. API keys can also use `file:<absolute-path>`. [Config reference](config.md)

Set the referenced variables in the process that loads the config. For provider requests from CLI or TUI, this is the **server**. Local `kuku config` commands read the current invocation's environment. If the running service was started before you set the variables, restart it when idle. [Configuration guide](../start/configuration.md)

## Provider trace logs

All builds default to full provider traces. Each record identifies the Session, Turn, and request so you can correlate the outbound request with the HTTP response, streamed events, or failure.

Full traces capture request bodies, request and response headers, raw SSE frames, and up to the first 4 KiB of non-success HTTP response bodies. Recognized credential fields and known request credential values are redacted, including where the provider echoes them. Non-credential content, including ordinary custom fields, prompts, images, tool results, and replies, remains visible. The trace is separate from the Session journal. For requests containing inline images, HTTP and stream errors in the Session use local messages instead of copying provider text that might echo image data. Generic error summaries use kuku's error classification and point to the Provider Trace when full tracing is active.

Use `KUKU_PROVIDER_TRACE=0` to turn traces off. SDK embedders can also choose lightweight diagnostics (protocol metadata without headers or content) via `SessionOptions::provider_trace` or `Query::provider_trace`. Traces cover inference requests, not subscription sign-in or token refresh exchanges.

Traces write to rotating JSONL segments under `$KUKU_HOME/logs/provider-trace/`. They obey [log retention](config.md#logs) and are capped at write time to `min(64 MiB, total log budget)`, with each segment capped at `min(8 MiB, trace budget)` (oldest segments evicted first).

## Hook process variables

When kuku runs a package hook, it injects:

| Variable | Meaning |
|---|---|
| `KUKU_SESSION_DIR` | Absolute path to the current Session directory |
| `KUKU_WORKSPACE` | Absolute path to the current workspace |
| `KUKU_PACKAGE_DIR` | Absolute path to the package root |

Hook processes also inherit `PATH`, `HOME`, `USERPROFILE` (Windows), `LANG`, and `LC_ALL`. Other environment variables and secrets are not forwarded automatically.
