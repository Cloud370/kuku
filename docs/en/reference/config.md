# Config

## Location

Default path:

```text
~/.kuku/config.toml
```

If `KUKU_HOME` is set, the CLI uses:

```text
$KUKU_HOME/config.toml
```

The HTTP server defaults to `config.toml` in its data directory and accepts an explicit `--config` path. An already running managed server reports its effective configuration with `kuku server status`. [Configuration](../start/configuration.md)

First-run setup, `kuku init`, and TUI `/model` use the same provider settings. See [Configuration](../start/configuration.md) for key storage and editing behavior.

The terminal client uses a separate preferences file. [Terminal preferences](#terminal-preferences)

## Top-Level Keys

| Key | Type | Meaning |
|---|---|---|
| `default_model` | string | Default tier name |
| `model.<name>` | table | Tier definition |
| `provider.<name>` | table | Provider definition |
| `discovery` | table | Agent and skill discovery settings |
| `handoff` | table | Long-session handoff settings |
| `logs` | table | Observability log retention settings |
| `plugin` | table | Package extension loading toggle |
| `update` | table | Update source and channel |

## `model.<name>`

Required and optional fields:

| Field | Type | Meaning |
|---|---|---|
| `provider` | string | Provider name from `provider.<name>` |
| `model` | string | Provider model ID |
| `think` | `provider-default`\|`off`\|`low`\|`medium`\|`high` | Thinking preference. `provider-default` omits the wire effort |
| `reasoning_effort` | string | Native reasoning value; overrides `think` |
| `service_tier` | string | Native service option, independent of reasoning |
| `context_window` | integer | Max input tokens |
| `max_output_tokens` | positive integer, optional | Generation ceiling for this tier, including reasoning tokens when the provider counts them as output |
| `purpose` | string | Route purpose shown to people and delegated Agents |
| `output` | `text`\|`image` | Requested Agent output. Defaults to text |

A single configured route can be the default. Set `default_model` to a route name, or omit it for a one-route configuration. Existing `strong`, `balanced`, and `light` routes still resolve. The TUI's explicit **Set current model as default** action reuses a matching tier or creates/overwrites one ordinary `user-default` tier when none matches. `user-default` has no special resolver priority; `default_model` selects it like any other tier.

On Chat and Responses, omit `max_output_tokens` to use the upstream default. Native Anthropic Messages requires a numeric value. The bundled Sonnet and Haiku presets set it in TOML. A custom Messages connection can take its value from its tier, the Agent or the host. Existing numeric settings continue to apply to their tier. The available context can further limit each response.

## `provider.<name>`

| Field | Type | Meaning |
|---|---|---|
| `format` | string | `anthropic`, `openai-chat`, or `openai-responses` |
| `base_url` | string | Provider API base URL with the protocol suffix appended |
| `endpoint_url` | string | Full request URL, used exactly when set |
| `models_url` | string | Optional full URL for standard model discovery |
| `proxy` | table | Connection network path for discovery, trials and model requests |
| `model_pricing` | table | USD accounting preferences keyed by exact model ID |
| `model_price_catalogs` | table | Public OpenRouter endpoint quotes saved by model setup |
| `model_capabilities` | table | Model-list declarations, manual overrides, and accepted thinking values keyed by exact model ID |
| `api_key` | string | `$ENV_VAR_NAME`, a literal key, or `file:<absolute-path>` |
| `auth` | `bearer`\|`x-api-key` | Override authentication independently of `format` |
| `profile` | string | `auto` (default), `openai`, `anthropic`, `deepseek`, `openrouter`, or `compatible` |
| `reasoning_efforts` | string[] | Override the declared native reasoning values for this connection's models |
| `service_tiers` | string[] | Override the declared native service values for this connection's models |
| `routing` | table | OpenRouter upstream routing options |
| `prompt_cache_scope` | `workspace`\|`session`\|`off` | Responses prompt-cache key scope; defaults to `workspace` |

For the built-in `openai` connection, `base_url` is `https://api.openai.com/v1`. The Responses adapter appends `/responses`. For a gateway, use the API base supplied by its operator. [OpenAI Responses API](https://developers.openai.com/api/reference/resources/responses/methods/create)

Use either `base_url` or `endpoint_url`. For example, the DeepSeek Chat base URLs `https://api.deepseek.com` and `https://api.deepseek.com/v1` append `/chat/completions` to their respective paths. An exact endpoint such as `https://gateway.example/gateway/infer` keeps its path. Use `models_url` for a separate catalog address such as `https://gateway.example/catalog/models`.

Set `proxy.mode` to `environment` (the default), `direct`, or `explicit`. For `explicit`, set `proxy.url` to an `http://`, `socks5://`, or `socks5h://` address. SOCKS5 resolves DNS on the client. SOCKS5H resolves it at the proxy. An authenticated proxy uses `proxy.auth.username` and one of `proxy.auth.password_env` or `proxy.auth.password_file`. The latter takes an absolute path and uses the same private-file checks as a model key.

Each `model_pricing` entry applies to the connection and exact model named in the table. The default `automatic` mode uses a provider receipt when available. It can estimate from OpenRouter quotes saved during setup when the response identifies one matching endpoint and service tier. Running setup again refreshes those quotes. The `configured` mode adds manual rates for a direct connection's standard tier. Use `routed` to enter separate rate rows for distinct serving endpoints or tiers. Use the provider's full endpoint tag when known and omit it for a direct connection. `usage-only` records tokens and leaves the monetary amount unknown.

Rates use USD per million tokens. Set `input`, `output`, `cache_read`, and `cache_write` as applicable. Use `cache_write_5m` and `cache_write_1h` when the provider reports separate write durations. The estimate keeps the selected rates and their source. Missing usage or several possible prices leaves the amount unknown.

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

A model capability entry has independent `discovered` and `manual` declarations. The supported fields are `reasoning_efforts`, `reasoning_default`, `input_modalities`, `output_modalities`, and `interfaces`. A `protocols` table can declare protocol-specific fields such as `anthropic_messages.system_prompt_update`. The resolver labels sources, keeps omitted values unknown, and preserves manual values across refreshes. A route with image input may offer [image inspection](tools.md). An Agent can save an image output setting when the route declares image output or leaves it unknown. Built-in adapters generate text and report an unsupported output error before dispatching image output requests. [DeepSeek model list](https://api-docs.deepseek.com/api/list-models/) · [OpenRouter model list](https://openrouter.ai/docs/api/api-reference/models/list-all-models-and-their-properties)

`prompt_cache_scope` only affects Responses requests. `workspace` hashes the normalized absolute workspace identity with the connection, endpoint and model, so Agents and Sessions in that workspace can share a key without sending the path. `session` derives a separate key from the same Session, Agent address and connection identity as transport correlation, adding the endpoint and model to isolate routes. `off` omits `prompt_cache_key`. If correlation context is unavailable, the adapter omits the key. None of these settings changes the transport correlation or OpenRouter `session_id`.

The SDK selects a connection by its configured name. Multiple connections may use the same `format`; selecting a protocol alone never picks the first matching connection. Explicit model settings override the selected tier. Choosing another connection does not carry native settings from an unrelated default tier; an explicitly selected tier still supplies its defaults and incompatible choices fail.

Known endpoint hostnames select their endpoint profile under `auto`. Custom gateways use `compatible` and require explicit capability declarations for native settings. Profiles describe documented adapter behavior, not confirmation from the upstream. Explicit unsupported reasoning or service values fail before dispatch. `think` is a portable tier preference. The adapter sends the mapped effort when the selected model and connection declare support for it. Otherwise the request uses the upstream default and records the preference with an empty effective effort. Direct DeepSeek maps `medium` to `high`. `reasoning_effort` is an exact native selection and is never silently remapped. Enabling a service tier does not change the model or reasoning.

Messages defaults to `x-api-key` authentication; OpenRouter uses Bearer authentication for all three protocols. DeepSeek Messages sends `thinking.type` as `enabled` or `disabled`, omits unsupported cache controls, and rejects service tiers. DeepSeek Responses sends complete stateless history and never uses `previous_response_id`. See the [DeepSeek thinking](https://api-docs.deepseek.com/guides/thinking_mode/), [Messages](https://api-docs.deepseek.com/guides/anthropic_api/), and [Responses](https://api-docs.deepseek.com/guides/responses_api/) contracts and [OpenRouter Messages authentication](https://openrouter.ai/docs/api/api-reference/anthropic-messages/create-a-message).

`routing.order` and `routing.only` contain upstream endpoint tags. `allow_fallbacks` controls upstream fallback; `require_parameters` defaults to `true` for both TOML and SDK construction. Optional `routing.max_price.prompt` and `.completion` cap USD per million tokens and do not cap total spend. OpenRouter receives a stable transport correlation derived from Session, Agent address, and connection name. Each Agent remains separate across Turns and reopen. Routing affinity does not guarantee cache retention. See the [routing](https://openrouter.ai/docs/guides/routing/provider-selection) and [prompt caching](https://openrouter.ai/docs/guides/best-practices/prompt-caching) contracts.

`api_key = "$NAME"` selects only that environment variable. `api_key = "file:/absolute/path"` selects only that file; relative file references are rejected, the file is validated and read at request time, and there is no source fallback. `file:` is a path reference, not a URL. `kuku config show` displays the reference without reading the secret. Keys stored by setup are plaintext (not encrypted) in one private file under `<data-dir>/credentials/`; the directory must be private and outside the workspace. See [File layout](file-layout.md) for Unix permissions and Windows ACL requirements. Existing key files are not automatically deleted.

Any other string config value whose first character is `$` is treated as an environment variable reference and resolved before validation.

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
| `threshold` | float | `0.7` |
| `keep_turns` | integer | `2` |

In a Session, `enabled` controls automatic context compaction. It can run when measured token usage crosses `threshold` or the request approaches its byte limit. The runtime checks that enough older material can be summarized and commits a successor only when it has usable savings. Manual `/compact` remains available. For a standalone SDK `Query`, `keep_turns` controls how many preceding turns remain after a model-generated handoff.

## `logs`

| Field | Type | Default |
|---|---|---|
| `max_age_days` | integer | `14` |
| `max_total_size_mb` | integer | `512` |

Observability logs are on by default and have no disable switch. kuku prunes logs by age first, then by total size budget. Pruning never touches session `events.jsonl` files. Opt-in provider trace files under `logs/provider-trace/` use the same retention budget.

## `plugin`

| Field | Type | Default |
|---|---|---|
| `enabled` | bool | `true` on new default configs |

This controls package-backed plugin loading from `.kuku/packages/` packages. When disabled, kuku does not load package-provided hooks or package-provided Skills.

## `update`

| Field | Type | Default |
|---|---|---|
| `source` | string | `github` |
| `channel` | string | `stable` |
| `sources` | table | empty |

Current documented values:

- `source = "github"` for built-in release manifests
- `source = "mirror"` when a custom mirror URL is selected
- `channel = "stable"` or `"alpha"`

Example:

```toml
[update]
source = "mirror"
channel = "alpha"

[update.sources]
custom = "https://example.com/latest.json"
```

## Default Config Example

The starter config file lives in `crates/kuku/assets/default-config.toml`.

## Terminal Preferences

The terminal reads `tui.toml` at startup from the local client data directory for both managed and explicit remote connections. The directory comes from `--data-dir`, then `KUKU_HOME`, then `~/.kuku`. SDK and service execution settings use `config.toml`.

Missing `tui.toml` or `fields` selects the built-in order `model,connection,agent,workspace,context,tps`. The default model and connection fields show the resolved model ID and service state. Explicitly selecting `model` or `connection` allows route details or the endpoint to appear when it fits. `execution` is an optional field.

Open `/statusline` from the action menu to edit the layout in the terminal. Use ↑ and ↓ to select a field, Space to toggle it, and ← and → to move enabled fields. The footer previews your changes. Press R to preview the built-in layout, Enter to save the layout to `tui.toml`, or Esc to discard the edits. Saving the built-in layout removes `statusline.fields`.

To choose all available fields, add this to `tui.toml`.

```toml
[statusline]
fields = [
  "model", "connection", "execution", "input", "agent", "workspace",
  "context", "tokens_in", "tokens_out", "cache", "cost", "tps",
]
```

List just the fields you want in display order. Use `fields = []` to hide the status line. Selecting `input` places input feedback in the status line. The default placement is in the editor. [Field meanings](cli.md#terminal-interface)

Set `KUKU_TUI_STATUSLINE` to a comma-separated list to override the file for that launch. Use `none` to hide the line or a blank value to use the built-in fields. With an override in use, `/statusline` still edits and saves the layout in `tui.toml`. The override resumes when you close the editor. The terminal reports an invalid active setting and falls back to the built-in fields. [Environment variables](environment-variables.md)
