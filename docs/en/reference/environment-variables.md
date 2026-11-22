# Environment Variables

## Runtime Variables

| Variable | Meaning |
|---|---|
| `KUKU_HOME` | Override the default runtime home directory |
| `KUKU_TUI_STATUSLINE` | Override the saved status line for this terminal launch with a comma-separated field list. [Terminal preferences](config.md#terminal-preferences) |
| `KUKU_TOKEN` | Authenticate a client using an explicitly selected server |
| `KUKU_SERVER_PASSWORD` | Set the bearer token for a server started with `kuku server` |
| `ANTHROPIC_API_KEY` | Common API key source for `provider.anthropic` |
| `OPENAI_API_KEY` | Common API key source for `provider.openai` |
| `DEEPSEEK_API_KEY` | Common API key source for `provider.deepseek` |
| `OPENROUTER_API_KEY` | Common API key source for `provider.openrouter` |
| `KUKU_PROVIDER_TRACE` | Set to `1` for full traces or `0` to disable provider traces |

`config.toml` can also reference environment variables with `$NAME` in string fields. For `api_key`, `$NAME` explicitly selects the environment source; `file:<absolute-path>` explicitly selects a private file. The sources never fall back to one another. Other string fields resolve during config load.

Set provider credentials and `KUKU_PROVIDER_TRACE` in the server's environment. An environment credential must be non-empty; a running server keeps its own environment when a new client connects. [Configuration](../start/configuration.md)

## Provider Trace Logs

Prerelease and untagged development binaries record protocol diagnostics by default. A stable release defaults to off. The diagnostic JSONL records contain Session, Turn and request correlation, model and route, HTTP status, SSE sequence and terminal markers, and failure kind. Reasoning details include type, format, index and request-local ID aliases. Arbitrary types and formats are also represented by request-local aliases. Diagnostic mode omits bodies, response content, headers and opaque reasoning payloads.

`KUKU_PROVIDER_TRACE=1` captures full request bodies, headers and raw SSE. Credential headers are redacted. The full trace can contain prompts, tool results, and model output. Embedded hosts can select an explicit policy using `SessionOptions::provider_trace` or `Query::provider_trace`.

Provider traces are stored as rotating JSONL segments under `$KUKU_HOME/logs/provider-trace/`. They share [log retention](config.md#logs) and are also limited at write time to 64 MiB or the configured total log budget, whichever is smaller. Segments are limited to 8 MiB or the trace budget, whichever is smaller. Older segments are evicted first.

## Default Behavior Without `KUKU_HOME`

If `KUKU_HOME` is unset, kuku uses:

```text
~/.kuku
```

## Hook Process Variables

When kuku runs a package hook, it sets these variables for the hook process:

| Variable | Meaning |
|---|---|
| `KUKU_SESSION_DIR` | Absolute path to the current session directory |
| `KUKU_WORKSPACE` | Absolute path to the current workspace |
| `KUKU_PACKAGE_DIR` | Absolute path to the package root |

The hook process also inherits `PATH`, `HOME`, `USERPROFILE` on Windows, `LANG`, and `LC_ALL`.

Other secrets are not passed through automatically.
