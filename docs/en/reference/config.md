# Config

## Location

Default path:

```text
~/.kuku/config.toml
```

If `KUKU_HOME` is set:

```text
$KUKU_HOME/config.toml
```

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
| `think` | `off`\|`low`\|`medium`\|`high` | Thinking level |
| `reasoning_effort` | string | Native reasoning value; overrides `think` |
| `service_tier` | string | Native service option, independent of reasoning |
| `context_window` | integer | Max input tokens |
| `max_output_tokens` | integer | Max output tokens |
| `purpose` | string | Human-readable tier summary |

Default tiers are `strong`, `balanced`, and `light`.

## `provider.<name>`

| Field | Type | Meaning |
|---|---|---|
| `format` | string | `anthropic`, `openai-chat`, or `openai-responses` |
| `base_url` | string | Provider API base URL, or `$ENV_VAR_NAME` |
| `api_key` | string | Literal key, or `$ENV_VAR_NAME` |
| `auth` | `bearer`\|`x-api-key` | Override authentication independently of `format` |
| `profile` | string | `auto` (default), `openai`, `anthropic`, `deepseek`, `openrouter`, or `compatible` |
| `reasoning_efforts` | string[] | Override the declared native reasoning values for this connection's models |
| `service_tiers` | string[] | Override the declared native service values for this connection's models |
| `routing` | table | OpenRouter upstream routing options |

The SDK selects a connection by its configured name. Multiple connections may use the same `format`; selecting a protocol alone never picks the first matching connection. Explicit model settings override the selected tier. Choosing another connection does not carry native settings from an unrelated default tier; an explicitly selected tier still supplies its defaults and incompatible choices fail.

Known endpoint hostnames select their endpoint profile under `auto`. Custom gateways use `compatible` and require explicit capability declarations for native settings. Profiles describe documented adapter behavior, not confirmation from the upstream. Explicit unsupported reasoning or service values fail before dispatch. `think` remains a portable tier preference: for direct DeepSeek, `medium` maps to native `high`; the requested and effective values are recorded separately. `reasoning_effort` is an exact native selection and is never silently remapped. Enabling a service tier does not change the model or reasoning.

Messages defaults to `x-api-key` authentication; OpenRouter uses Bearer authentication for all three protocols. DeepSeek Messages sends `thinking.type` as `enabled` or `disabled`, omits unsupported cache controls, and rejects service tiers. DeepSeek Responses sends complete stateless history and never uses `previous_response_id`. See the [DeepSeek thinking](https://api-docs.deepseek.com/guides/thinking_mode/), [Messages](https://api-docs.deepseek.com/guides/anthropic_api/), and [Responses](https://api-docs.deepseek.com/guides/responses_api/) contracts and [OpenRouter Messages authentication](https://openrouter.ai/docs/api/api-reference/anthropic-messages/create-a-message).

`routing.order` and `routing.only` contain upstream endpoint tags. `allow_fallbacks` controls upstream fallback; `require_parameters` defaults to `true` for both TOML and SDK construction. Optional `routing.max_price.prompt` and `.completion` cap USD per million tokens and do not cap total spend. OpenRouter receives a stable transport correlation derived from Session, Agent address, and connection name. Each Agent remains separate across Turns and reopen. Routing affinity does not guarantee cache retention. See the [routing](https://openrouter.ai/docs/guides/routing/provider-selection) and [prompt caching](https://openrouter.ai/docs/guides/best-practices/prompt-caching) contracts.

Any string config value whose first character is `$` is treated as an environment variable reference and resolved before validation, except `api_key`, which keeps the env-var reference and resolves later when the provider is used.

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

When estimated context usage crosses `threshold`, kuku injects a handoff instruction. The summary is persisted only if the model returns a handoff document; future context then keeps only the most recent `keep_turns` turns before that boundary.

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
