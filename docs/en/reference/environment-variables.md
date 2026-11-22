# Environment variables

## Runtime variables

| Variable | Meaning |
|---|---|
| `KUKU_HOME` | Override the default data directory (`~/.kuku`) |
| `KUKU_TUI_STATUSLINE` | Comma-separated status-line fields for this TUI launch (`none` hides it). [Terminal preferences](config.md#terminal-preferences) |
| `KUKU_TOKEN` | Bearer token for connecting to an explicit `--remote` server |
| `KUKU_SERVER_PASSWORD` | Bearer token required by a server started via `kuku server` |
| `ANTHROPIC_API_KEY` | Default API key variable for `provider.anthropic` |
| `OPENAI_API_KEY` | Default API key variable for `provider.openai` |
| `DEEPSEEK_API_KEY` | Default API key variable for `provider.deepseek` |
| `OPENROUTER_API_KEY` | Default API key variable for `provider.openrouter` |
| `KUKU_PROVIDER_TRACE` | `1` enables full provider traces; `0` disables traces |

Any string field in `config.toml` starting with `$` resolves that environment variable on load, except `api_key`, which keeps the `$NAME` reference and resolves it when calling the provider. `$NAME` and `file:<absolute-path>` are mutually exclusive and never fall back to each other.

Provider keys and `KUKU_PROVIDER_TRACE` are read from the **server** process environment (env keys must be non-empty). New clients connecting to an already-running server inherit the server's environment. [Configuration](../start/configuration.md)

## Provider trace logs

Prerelease and untagged development builds record lightweight protocol diagnostics by default; stable releases default to off. Diagnostic JSONL records capture Session/Turn/request correlation, model and route, HTTP status, SSE sequence and terminal markers, failure kind, and anonymized reasoning-entry metadata (omitting request/response bodies, headers, and opaque payloads).

Setting `KUKU_PROVIDER_TRACE=1` captures full request bodies, headers, and raw SSE frames. Secret headers are redacted, but prompts, tool results, and replies remain in the body. SDK embedders can configure this explicitly via `SessionOptions::provider_trace` or `Query::provider_trace`.

Traces write to rotating JSONL segments under `$KUKU_HOME/logs/provider-trace/`. They obey [log retention](config.md#logs) and are capped at write time to `min(64 MiB, total log budget)`, with each segment capped at `min(8 MiB, trace budget)` (oldest segments evicted first).

## Hook process variables

When kuku runs a package hook, it injects:

| Variable | Meaning |
|---|---|
| `KUKU_SESSION_DIR` | Absolute path to the current Session directory |
| `KUKU_WORKSPACE` | Absolute path to the current workspace |
| `KUKU_PACKAGE_DIR` | Absolute path to the package root |

Hook processes also inherit `PATH`, `HOME`, `USERPROFILE` (Windows), `LANG`, and `LC_ALL`. Other environment variables and secrets are not forwarded automatically.
