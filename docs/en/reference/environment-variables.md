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
| `KUKU_PROVIDER_TRACE` | Set to `1` to write provider request and response trace logs |

`config.toml` can also reference environment variables with `$NAME` in string fields. For `api_key`, `$NAME` explicitly selects the environment source; `file:<absolute-path>` explicitly selects a private file. The sources never fall back to one another. Other string fields resolve during config load.

Set provider credentials and `KUKU_PROVIDER_TRACE` in the server's environment. An environment credential must be non-empty; a running server keeps its own environment when a new client connects. [Configuration](../start/configuration.md)

## Provider Trace Logs

`KUKU_PROVIDER_TRACE=1` enables provider diagnostics for real API requests. When enabled, kuku writes JSONL trace files under:

```text
$KUKU_HOME/logs/provider-trace/<yyyy-mm-dd>/<session-id>.jsonl
```

Provider trace records include request headers, request bodies, response headers, and streamed response events. Secret header values such as `authorization`, `x-api-key`, `api-key`, cookies, and proxy credentials are redacted before writing. Request and response bodies may still contain prompt text, tool results, model output, or other task data, so enable this only while debugging.

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
