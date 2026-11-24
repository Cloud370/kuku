# Config

## Location

The default path is `config.toml` in the data directory selected by `--data-dir`, `KUKU_HOME`, or `~/.kuku`, in that order.

The server accepts `--config PATH`. Check the file a running service has loaded with `kuku server status`. First-run setup and `/model` edit the local config. [Configuration guide](../start/configuration.md) · [CLI path selection](cli.md#kuku-config) · [Terminal preferences](#terminal-preferences)

## Minimal config

```toml
default_model = { connection = "work", model = "your-model-id" }

[provider.work]
format = "openai-responses"
base_url = "https://gateway.example/v1"
api_key = "$MY_GATEWAY_KEY"
```

Replace the URL and model ID with the ones your connection offers. Set `MY_GATEWAY_KEY` in the service environment or change the reference to a variable you choose. The required `default_model` pair registers that model on its connection, so this config can run without a separate model table. [Environment references](environment-variables.md#config-references)

When you change the default through the TUI or a CLI patch, kuku keeps the previous model registered if its connection remains and you have not explicitly deleted that model. For a manual edit, add a table such as `[model.old_connection."old-model"]` before changing `default_model` to keep the previous pair available to existing Sessions. Removing a model requires affected Agents to select another configured pair.

## Top-level keys

| Key | Type | Meaning |
|---|---|---|
| `default_model` | table | Required default `connection` and exact `model` ID for new Sessions |
| `model.<connection>."<model-id>"` | table | One registered model's settings |
| `provider.<name>` | table | Provider connection definition |
| `discovery` | table | Agent and Skill discovery settings |
| `handoff` | table | Context compaction settings |
| `logs` | table | Observability log retention settings |
| `plugin` | table | Package extension loading toggle |
| `update` | table | Update source and channel |

## Model settings

Add a table for each additional model, or to set options on the default pair. Quote model IDs that contain dots, slashes, or other TOML key punctuation. A model ID can appear on multiple connections, each with its own settings.

```toml
[model.work."your-model-id"]
context_window = 200000
max_output_tokens = 8192

[model.work."another-model-id"]
max_output_tokens = 4096
```

| Field | Type | Meaning |
|---|---|---|
| `think` | `provider-default`\|`off`\|`low`\|`medium`\|`high` | Portable thinking preference (defaults to `provider-default`) |
| `reasoning_effort` | string | Native reasoning value; overrides `think` |
| `service_tier` | string | Native service option, independent of reasoning |
| `context_window` | integer | Max input tokens (defaults to 200,000) |
| `max_output_tokens` | positive integer, optional | Generation ceiling (including reasoning tokens when counted as output) |
| `purpose` | string | Route description shown to humans and delegated Agents |
| `output` | `text`\|`image` | Requested Agent output (defaults to `text`) |
| `request` | table | Model route overrides for [request headers and body](#request-headers-and-body) |

On Chat and Responses, omitting `max_output_tokens` uses the upstream default. Native Anthropic Messages requires a numeric value from the model settings, Agent, or host. Available context can still cap individual responses.

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
| `model_capabilities` | table | Model-list declarations, manual overrides, tested thinking values, and custom service tiers keyed by exact model ID |
| `api_key` | string | `$ENV_VAR_NAME`, literal key, or `file:<absolute-path>` |
| `subscription` | table | Subscription account reference with `channel` and absolute `credential_file` |
| `auth` | `bearer`\|`x-api-key` | Override auth header independently of `format` |
| `profile` | string | `auto` (default), `openai`, `anthropic`, `deepseek`, `openrouter`, or `compatible` |
| `reasoning_efforts` | string[] | Override declared native reasoning values for this connection |
| `service_tiers` | string[] | Override declared native service values for this connection |
| `routing` | table | OpenRouter upstream routing options |
| `prompt_cache_scope` | `workspace`\|`session`\|`off` | Responses prompt-cache key scope (defaults to `workspace`) |
| `request` | table | Connection-level [request headers and body](#request-headers-and-body) |

### Endpoints and proxies

- Set either `base_url` or `endpoint_url`. Responses appends `/responses` to a base URL like `https://api.openai.com/v1`. Chat Completions appends `/chat/completions`. The adapter uses `endpoint_url` verbatim. Use `models_url` when model listing lives at a separate address. [OpenAI Responses API](https://developers.openai.com/api/reference/resources/responses/methods/create)
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

Each model capability entry may also contain a `custom_service_tiers` table. Its keys are native tier names and its values are `unverified` for user declarations and `confirmed` when a probe reported the requested tier. These names extend the discovered or manually overridden service tier list. For example, `provider.gateway.model_capabilities."example/model".custom_service_tiers.turbo = "unverified"` adds `turbo` to that connection and exact model.

- **Capabilities**: Each `model_capabilities.<model>` entry separates `discovered` and `manual` blocks (`reasoning_efforts`, `reasoning_default`, `input_modalities`, `output_modalities`, `interfaces`, plus protocol-specific tables such as `protocols.anthropic_messages.system_prompt_update`). Manual overrides persist across refreshes; omitted fields remain unknown. Image-input routes may offer [image inspection](tools.md); built-in text adapters reject image-output requests before dispatch. [DeepSeek model list](https://api-docs.deepseek.com/api/list-models/) · [OpenRouter model list](https://openrouter.ai/docs/api/api-reference/models/list-all-models-and-their-properties)
- **Profiles and reasoning**: Under `profile = "auto"`, known hosts pick their endpoint profile; custom gateways use `compatible` and require explicit capability declarations for native options. Connections are selected by name (never by first matching `format`). `think` maps to a declared effort or falls back to upstream default; direct DeepSeek maps `medium` to `high`. `reasoning_effort` is an exact native value (never remapped; unsupported values fail before dispatch).
- **Provider specifics**: Messages defaults to `x-api-key`; OpenRouter uses Bearer auth across all three protocols. DeepSeek Messages sends `thinking.type` (`enabled`/`disabled`), omits unsupported cache controls, and rejects service tiers. DeepSeek Responses stays stateless and never sends `previous_response_id`. [DeepSeek thinking](https://api-docs.deepseek.com/guides/thinking_mode/) · [DeepSeek Messages](https://api-docs.deepseek.com/guides/anthropic_api/) · [DeepSeek Responses](https://api-docs.deepseek.com/guides/responses_api/) · [OpenRouter Messages auth](https://openrouter.ai/docs/api/api-reference/anthropic-messages/create-a-message)
- **OpenRouter routing**: `routing.order` / `routing.only` list endpoint tags; `allow_fallbacks` controls fallback; `require_parameters` defaults to `true`; `routing.max_price.prompt` / `.completion` cap per-million-token prices (not total spend). OpenRouter gets a stable per-Session/Agent/connection correlation ID (affinity does not guarantee cache retention). [OpenRouter routing](https://openrouter.ai/docs/guides/routing/provider-selection) · [Prompt caching](https://openrouter.ai/docs/guides/best-practices/prompt-caching)

### Request headers and body

`request.headers` maps HTTP header names to strings. `request.body` maps top-level JSON keys to TOML values, including arrays and tables. These settings apply to built-in inference. Connections also use their request settings during model trials.

Bundled TOML presets, connection settings, and matching model route settings apply in that order. Headers match case-insensitively. A body key replaces the whole value, including nested objects such as OpenRouter's `provider`. Set `request.inherit = false` at either level to start with empty request fields at that level.

```toml
[provider.openrouter.request.headers]
"X-Title" = "My workspace"

[provider.openrouter.request.body.provider]
order = ["anthropic"]
allow_fallbacks = false
require_parameters = true
data_collection = "deny"
zdr = true

[model.openrouter."anthropic/claude-haiku-4.5".request.body]
cache_control = { type = "ephemeral", ttl = "1h" }
```

Add these tables to an existing OpenRouter connection and model route. The `request.body.provider` object takes precedence over the connection's `routing` table. OpenRouter defines the native routing and data policy fields. [OpenRouter provider routing](https://openrouter.ai/docs/guides/routing/provider-selection)

- `"{{session_id}}"` resolves to an opaque value stable for the Session, Agent, and connection. `"{{prompt_cache_key}}"` resolves to the key selected by `prompt_cache_scope`. Fields whose values are unavailable are omitted.
- `"{{request_id}}"` is a header-only placeholder for a fresh HTTP attempt identifier. Placeholders replace complete string values, including strings nested inside body objects.
- Header values can be `"$NAME"` references. The service reads them when preparing a request. Provider request traces redact configured header values and custom body fields. Strings in `request.body` are literal apart from the placeholders above.
- The adapter owns credential and HTTP framing headers, model and message content, tool definitions, streaming, stateful replay fields, and resolved reasoning, service tier, and output-limit settings. Configuring one of these reserved fields produces a configuration error.
- ChatGPT sign-in rejects explicitly configured body fields the preview excludes, such as `metadata`, `temperature`, and `prompt_cache_retention`, before sending the request. [Subscription request limits](https://developers.openai.com/siwc/token-sharing-open-source/preview-limitations)

### Prompt caching and credentials

| Channel | Request defaults and cache behavior |
|---|---|
| ChatGPT subscription | Requests send a stable `session-id` to help find the cache and a fresh `X-Client-Request-Id` to track each HTTP attempt. Requests carry `prompt_cache_key` or `x-session-id` only if the connection or model route explicitly adds them. [Codex session routing](https://github.com/openai/codex/pull/44862) |
| OpenAI API-key Responses | Prefix caching is automatic. Models before GPT-5.6 receive `prompt_cache_key` when available to help cache routing. On GPT-5.6 and later, a configured key groups cache accounting. [OpenAI prompt caching](https://developers.openai.com/api/docs/guides/prompt-caching) |
| OpenRouter | Body `session_id` helps keep the selected upstream stable. Cache behavior follows that upstream and model. Claude routes also receive top-level `cache_control`. [OpenRouter prompt caching](https://openrouter.ai/docs/guides/best-practices/prompt-caching) |
| Native Anthropic Messages | Top-level `cache_control = { type = "ephemeral" }` enables automatic caching. The breakpoint advances as the conversation grows. [Anthropic prompt caching](https://platform.claude.com/docs/en/build-with-claude/prompt-caching) |

Reuse requires a matching prefix and a reachable cache entry. Keep the model, tools, system instructions, and already submitted messages stable, then append new input. Cold requests, expired entries, and changes to the prefix or serving route can produce zero cache reads. An occasional zero is also possible during a successful turn with the same session and prefix. [OpenAI prompt caching](https://developers.openai.com/api/docs/guides/prompt-caching) · [Codex cache observations](https://github.com/openai/codex/issues/44716)

Read `usage.input_tokens_details.cached_tokens` for Responses, `usage.prompt_tokens_details.cached_tokens` for Chat Completions, and `usage.cache_read_input_tokens` for Anthropic Messages. Messages reports newly written tokens separately as `usage.cache_creation_input_tokens`. An omitted usage field leaves the read count unknown.

Anthropic's default cache lifetime is five minutes. Haiku 5.5 cache writes cost 1.25 times the base input price at five minutes and 2 times at one hour. Cache reads cost 0.1 times the base input price for either lifetime. [Anthropic prompt caching prices](https://platform.claude.com/docs/en/build-with-claude/prompt-caching)

Model trials and one-shot requests such as web-page summarization send `cache_control` only if explicitly configured. Requests that recover from output truncation retain their session identity. Stateless OpenAI Responses receive encrypted reasoning from the provider by default. [OpenAI stateless reasoning](https://developers.openai.com/api/docs/guides/reasoning#preserve-reasoning-without-stored-responses)

- **`prompt_cache_scope`** (Responses only) controls the `{{prompt_cache_key}}` placeholder. `workspace` (default) hashes the normalized workspace path, connection, endpoint and model. `session` hashes Session, Agent, connection, endpoint and model. `off` leaves the placeholder unresolved.
- **`api_key`**: `$NAME` reads only that env var; `file:/absolute/path` validates and reads only that file at request time (no relative paths, no URL syntax, no fallback between sources). Setup writes unencrypted keys under `<data-dir>/credentials/`; `kuku config show` prints only the reference. [File layout](file-layout.md)
- See [Environment variables](environment-variables.md) for when `$NAME` references resolve.

A subscription connection uses its account reference as the credential source. ChatGPT subscriptions use the official OpenAI Responses and models endpoints. The connection's network mode also applies to the account's authentication requests. Explicit SOCKS proxies use `socks5h://` for proxy-side DNS. [Subscription sign-in](../start/configuration.md#sign-in-to-a-subscription-on-a-headless-host)

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

The starter template contains optional runtime settings and a commented example connection. [Starter template](../../../crates/kuku/assets/default-config.toml)

## Terminal preferences

The TUI reads `tui.toml` from the local client data directory (`--data-dir`, then `KUKU_HOME`, then `~/.kuku`) for both local and remote connections. Execution settings stay in `config.toml`.

If `tui.toml` or `statusline.fields` is absent, the built-in order is `model,agent,workspace,context,cache,tps,tokens_in,tokens_out,cost`. An explicit `model` can show its route when space permits. The optional `connection` field includes healthy connections as local or remote. `execution` and `permission_mode` are optional. `permission_mode` reports the workspace permission mode.

Run `/statusline` in the TUI to toggle (`Space`), reorder (`←`/`→`), preview defaults (`R`), and save (`Enter`) directly to `tui.toml` (saving defaults removes `statusline.fields`).

Example with all supported fields:

```toml
[statusline]
fields = [
  "model", "connection", "execution", "input", "agent", "workspace",
  "context", "tokens_in", "tokens_out", "cache", "cost", "tps", "permission_mode",
]
```

Set `fields = []` to hide the status line. Including `input` moves input feedback from the editor to the status line. [Field meanings](cli.md#terminal-interface)

Set `KUKU_TUI_STATUSLINE` to a comma-separated list to override `tui.toml` for one launch (`none` hides it; blank uses defaults; invalid settings fall back to defaults). [Environment variables](environment-variables.md)

Use `/notifications` to select with ↑ and ↓, toggle with Space and save with Enter. The `[notifications]` keys in `tui.toml` are `question`, `permission`, `interrupted`, and `completed`. Each defaults to `true`. The TUI sends BEL for new events. The terminal controls whether BEL produces sound, a flash or silence. History and replay on reconnect stay quiet.
