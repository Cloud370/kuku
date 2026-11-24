# Config

## Location

Default path is `~/.kuku/config.toml`, or `$KUKU_HOME/config.toml` when `KUKU_HOME` is set.

The HTTP server defaults to `config.toml` inside its data directory and accepts `--config <path>`. Check the active config of a running managed service with `kuku server status`.

First-run setup, `kuku init`, and TUI `/model` all edit this file. [Configuration](../start/configuration.md) · [Terminal preferences](#terminal-preferences)

## Top-level keys

| Key | Type | Meaning |
|---|---|---|
| `default_model` | string | Default tier name |
| `model.<name>` | table | Tier definition |
| `provider.<name>` | table | Provider connection definition |
| `discovery` | table | Agent and Skill discovery settings |
| `handoff` | table | Context compaction settings |
| `logs` | table | Observability log retention settings |
| `plugin` | table | Package extension loading toggle |
| `update` | table | Update source and channel |

## `model.<name>`

| Field | Type | Meaning |
|---|---|---|
| `provider` | string | Provider name from `provider.<name>` |
| `model` | string | Provider model ID |
| `think` | `provider-default`\|`off`\|`low`\|`medium`\|`high` | Portable thinking preference (`provider-default` omits the wire effort) |
| `reasoning_effort` | string | Native reasoning value; overrides `think` |
| `service_tier` | string | Native service option, independent of reasoning |
| `context_window` | integer | Max input tokens |
| `max_output_tokens` | positive integer, optional | Generation ceiling (including reasoning tokens when counted as output) |
| `purpose` | string | Route description shown to humans and delegated Agents |
| `output` | `text`\|`image` | Requested Agent output (defaults to `text`) |

Set `default_model` to a tier name, or omit it when only one tier is configured. Legacy `strong`, `balanced`, and `light` tiers still resolve. In the TUI, **Set current model as default** reuses a matching tier or writes a normal `user-default` tier selected via `default_model`.

On Chat and Responses, omitting `max_output_tokens` uses the upstream default. Native Anthropic Messages requires a numeric value (set in the bundled Sonnet/Haiku presets; custom Messages routes take it from the tier, Agent, or host). Available context can still cap individual responses.

## `provider.<name>`

| Field | Type | Meaning |
|---|---|---|
| `format` | string | `anthropic`, `openai-chat`, or `openai-responses` |
| `base_url` | string | API base URL (protocol path suffix is appended) |
| `endpoint_url` | string | Exact request URL, used verbatim when set |
| `models_url` | string | Optional full URL for model discovery |
| `proxy` | table | Network proxy settings for discovery, trials, and requests |
| `model_pricing` | table | USD accounting settings keyed by exact model ID |
| `model_price_catalogs` | table | Public OpenRouter quotes saved during setup |
| `model_capabilities` | table | Discovered/manual capability overrides keyed by exact model ID |
| `api_key` | string | `$ENV_VAR_NAME`, literal key, or `file:<absolute-path>` |
| `subscription` | table | Subscription account reference with `channel` and absolute `credential_file` |
| `auth` | `bearer`\|`x-api-key` | Override auth header independently of `format` |
| `profile` | string | `auto` (default), `openai`, `anthropic`, `deepseek`, `openrouter`, or `compatible` |
| `reasoning_efforts` | string[] | Override declared native reasoning values for this connection |
| `service_tiers` | string[] | Override declared native service values for this connection |
| `routing` | table | OpenRouter upstream routing options |
| `prompt_cache_scope` | `workspace`\|`session`\|`off` | Responses prompt-cache key scope (defaults to `workspace`) |

### Endpoints and proxies

- Set either `base_url` or `endpoint_url`. For example, built-in `openai` uses `https://api.openai.com/v1` (Responses appends `/responses`; DeepSeek Chat appends `/chat/completions` to `https://api.deepseek.com` or `/v1`). Exact `endpoint_url` values are never suffixed. Use `models_url` when model listing lives at a separate address. [OpenAI Responses API](https://developers.openai.com/api/reference/resources/responses/methods/create)
- `proxy.mode` accepts `environment` (default), `direct`, or `explicit`. Explicit mode requires `proxy.url` (`http://`, `socks5://` for client DNS, or `socks5h://` for proxy DNS). Optional proxy auth takes `proxy.auth.username` plus either `proxy.auth.password_env` or `proxy.auth.password_file` (absolute path, validated like credential files).

### Pricing and cost estimation

- `model_pricing.<model>.mode` defaults to `automatic` (uses provider billing receipts when available, or OpenRouter setup quotes when a response matches one endpoint and service tier; re-running setup refreshes quotes).
- `configured` sets manual USD-per-million-token rates (`input`, `output`, `cache_read`, `cache_write`, or `cache_write_5m` / `cache_write_1h`) for a direct standard tier.
- `routed` defines separate `routes` rows per serving endpoint tag or `service_tier`.
- `usage-only` tracks tokens while leaving cost unknown. Missing usage or ambiguous prices also leave cost unknown.

```toml
[provider.gateway.proxy]
mode = "explicit"
url = "socks5h://127.0.0.1:1080"

[provider.gateway.model_pricing."example/model"]
mode = "configured"
input = 0.2
output = 1.0
cache_read = 0.05

[provider.direct.model_pricing."example/model"]
mode = "routed"

[[provider.direct.model_pricing."example/model".routes]]
service_tier = "priority"
input = 0.4
output = 2.0
cache_read = 0.1
```

### Capabilities, profiles, and routing

- **Capabilities**: Each `model_capabilities.<model>` entry separates `discovered` and `manual` blocks (`reasoning_efforts`, `reasoning_default`, `input_modalities`, `output_modalities`, `interfaces`, plus protocol-specific tables such as `protocols.anthropic_messages.system_prompt_update`). Manual overrides persist across refreshes; omitted fields remain unknown. Image-input routes may offer [image inspection](tools.md); built-in text adapters reject image-output requests before dispatch. [DeepSeek model list](https://api-docs.deepseek.com/api/list-models/) · [OpenRouter model list](https://openrouter.ai/docs/api/api-reference/models/list-all-models-and-their-properties)
- **Profiles and reasoning**: Under `profile = "auto"`, known hosts pick their endpoint profile; custom gateways use `compatible` and require explicit capability declarations for native options. Connections are selected by name (never by first matching `format`). `think` maps to a declared effort or falls back to upstream default; direct DeepSeek maps `medium` to `high`. `reasoning_effort` is an exact native value (never remapped; unsupported values fail before dispatch).
- **Provider specifics**: Messages defaults to `x-api-key`; OpenRouter uses Bearer auth across all three protocols. DeepSeek Messages sends `thinking.type` (`enabled`/`disabled`), omits unsupported cache controls, and rejects service tiers. DeepSeek Responses stays stateless and never sends `previous_response_id`. [DeepSeek thinking](https://api-docs.deepseek.com/guides/thinking_mode/) · [DeepSeek Messages](https://api-docs.deepseek.com/guides/anthropic_api/) · [DeepSeek Responses](https://api-docs.deepseek.com/guides/responses_api/) · [OpenRouter Messages auth](https://openrouter.ai/docs/api/api-reference/anthropic-messages/create-a-message)
- **OpenRouter routing**: `routing.order` / `routing.only` list endpoint tags; `allow_fallbacks` controls fallback; `require_parameters` defaults to `true`; `routing.max_price.prompt` / `.completion` cap per-million-token prices (not total spend). OpenRouter gets a stable per-Session/Agent/connection correlation ID (affinity does not guarantee cache retention). [OpenRouter routing](https://openrouter.ai/docs/guides/routing/provider-selection) · [Prompt caching](https://openrouter.ai/docs/guides/best-practices/prompt-caching)

### Prompt caching and credentials

- **`prompt_cache_scope`** (Responses only): `workspace` (default) hashes the normalized workspace path + connection + endpoint + model so Sessions in the same workspace share a key without leaking paths; `session` isolates by Session + Agent + connection + endpoint + model; `off` omits `prompt_cache_key`. Does not affect OpenRouter `session_id`.
- **`api_key`**: `$NAME` reads only that env var; `file:/absolute/path` validates and reads only that file at request time (no relative paths, no URL syntax, no fallback between sources). Setup writes unencrypted keys under `<data-dir>/credentials/`; `kuku config show` prints only the reference. [File layout](file-layout.md)
- Any other string config value starting with `$` resolves that env var before validation.

A subscription connection uses its account reference as the credential source. ChatGPT subscriptions use the official OpenAI Responses and models endpoints and require an explicit HTTP or SOCKS5H proxy. The connection's proxy also applies to the account's authentication requests. [Subscription sign-in](../start/configuration.md#sign-in-to-a-subscription-on-a-headless-host)

## `discovery`

| Field | Type | Default |
|---|---|---|
| `auto_discover` | bool | `true` |
| `extra_user_paths` | string[] | `[]` |
| `extra_project_paths` | string[] | `[]` |

`auto_discover` scans common user and project dot-directories for `skills`, `agents`, and `agent` subdirectories.

## `handoff`

| Field | Type | Default |
|---|---|---|
| `enabled` | bool | `true` |
| `threshold` | float | `0.95` |
| `keep_turns` | integer | `2` |

The `threshold` is the share of available input tokens that triggers automatic compaction. Set it between `0.0` and `1.0`. The runtime accounts for reasoning overhead and reserves bounded output room before applying the threshold. The model's generation ceiling applies separately. It commits a new context segment when the summary frees usable space. After a failed automatic attempt, it waits for additional material before repeating that attempt. Manual `/compact` remains available. For a standalone SDK `Query`, `keep_turns` sets how many recent turns remain after a handoff.

## `logs`

| Field | Type | Default |
|---|---|---|
| `max_age_days` | integer | `14` |
| `max_total_size_mb` | integer | `512` |

Observability logs are always enabled and pruned by age first, then total size. Pruning never touches Session `events.jsonl` files. [Provider trace limits](environment-variables.md#provider-trace-logs)

## `plugin`

| Field | Type | Default |
|---|---|---|
| `enabled` | bool | `true` on new default configs |

Enables or disables loading hooks and Skills from `.kuku/packages/`.

## `update`

| Field | Type | Default |
|---|---|---|
| `source` | string | `github` |
| `channel` | string | `stable` |
| `sources` | table | empty |

`source` accepts `github` (built-in manifests) or `mirror` (custom URL); `channel` accepts `stable` or `alpha`.

```toml
[update]
source = "mirror"
channel = "alpha"

[update.sources]
custom = "https://example.com/latest.json"
```

## Default config example

Starter template: `crates/kuku/assets/default-config.toml`.

## Terminal preferences

The TUI reads `tui.toml` from the local client data directory (`--data-dir`, then `KUKU_HOME`, then `~/.kuku`) for both local and remote connections. Execution settings stay in `config.toml`.

If `tui.toml` or `statusline.fields` is absent, the built-in order is `model,agent,workspace,context,cache,tps,tokens_in,tokens_out,cost`. An explicit `model` can show its route when space permits. The optional `connection` field includes healthy connections as local or remote. `execution` is optional.

Run `/statusline` in the TUI to toggle (`Space`), reorder (`←`/`→`), preview defaults (`R`), and save (`Enter`) directly to `tui.toml` (saving defaults removes `statusline.fields`).

Example with all supported fields:

```toml
[statusline]
fields = [
  "model", "connection", "execution", "input", "agent", "workspace",
  "context", "tokens_in", "tokens_out", "cache", "cost", "tps",
]
```

Set `fields = []` to hide the status line. Including `input` moves input feedback from the editor to the status line. [Field meanings](cli.md#terminal-interface)

Set `KUKU_TUI_STATUSLINE` to a comma-separated list to override `tui.toml` for one launch (`none` hides it; blank uses defaults; invalid settings fall back to defaults). [Environment variables](environment-variables.md)
