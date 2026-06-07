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

Observability logs are on by default and have no disable switch. kuku prunes logs by age first, then by total size budget. Pruning never touches session `events.jsonl` files.

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
