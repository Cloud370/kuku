# Config

## Location

默认路径：

```text
~/.kuku/config.toml
```

如果设置了 `KUKU_HOME`：

```text
$KUKU_HOME/config.toml
```

## 顶层键

| Key | Type | Meaning |
|---|---|---|
| `default_model` | string | 默认 tier 名称 |
| `model.<name>` | table | tier 定义 |
| `provider.<name>` | table | provider 定义 |
| `discovery` | table | Agent 和 Skill 的发现设置 |
| `handoff` | table | 长 Session 的 handoff 设置 |
| `logs` | table | 可观测性日志保留设置 |
| `plugin` | table | package 扩展加载开关 |
| `update` | table | 更新源和 channel |

## `model.<name>`

必填和可选字段：

| Field | Type | Meaning |
|---|---|---|
| `provider` | string | `provider.<name>` 中的 provider 名称 |
| `model` | string | provider 模型 ID |
| `think` | `off`\|`low`\|`medium`\|`high` | thinking 等级 |
| `context_window` | integer | 最大输入 token 数 |
| `max_output_tokens` | integer | 最大输出 token 数 |
| `purpose` | string | 面向人的 tier 摘要 |

默认 tier 为 `strong`、`balanced` 和 `light`。

## `provider.<name>`

| Field | Type | Meaning |
|---|---|---|
| `format` | string | `anthropic`、`openai-chat` 或 `openai-responses` |
| `base_url` | string | provider API 基础 URL，或 `$ENV_VAR_NAME` |
| `api_key` | string | 直接填写的 key，或 `$ENV_VAR_NAME` |

任何第一个字符为 `$` 的字符串配置值都会被视为环境变量引用，并在校验前解析；但 `api_key` 会保留环境变量引用，并在使用 provider 时再解析。

## `discovery`

| Field | Type | Default |
|---|---|---|
| `auto_discover` | bool | `true` |
| `extra_user_paths` | string[] | `[]` |
| `extra_project_paths` | string[] | `[]` |

`auto_discover` 会扫描常见的用户级和项目级点目录中的 `skills`、`agents` 和 `agent` 子目录。

## `handoff`

| Field | Type | Default |
|---|---|---|
| `enabled` | bool | `true` |
| `threshold` | float | `0.7` |
| `keep_turns` | integer | `2` |

当估算的上下文使用量超过 `threshold` 时，kuku 会注入 handoff 指令。只有当模型返回 handoff 文档时，摘要才会被持久化；之后的上下文会在该边界前只保留最近的 `keep_turns` 轮。

## `logs`

| Field | Type | Default |
|---|---|---|
| `max_age_days` | integer | `14` |
| `max_total_size_mb` | integer | `512` |

可观测性日志默认开启，且没有禁用开关。kuku 会先按时间清理日志，再按总大小预算清理。清理日志永远不会触碰 Session 的 `events.jsonl` 文件。

## `plugin`

| Field | Type | Default |
|---|---|---|
| `enabled` | bool | 新默认配置中为 `true` |

这个开关控制 `.kuku/packages/` 中基于 package 的 plugin 加载。关闭后，kuku 不会加载 package 提供的 hook，也不会加载 package 提供的 Skill。

## `update`

| Field | Type | Default |
|---|---|---|
| `source` | string | `github` |
| `channel` | string | `stable` |
| `sources` | table | empty |

当前文档中的可用值：

- `source = "github"` 表示使用内建发布 manifest
- `source = "mirror"` 表示选择了自定义镜像 URL
- `channel = "stable"` 或 `"alpha"`

示例：

```toml
[update]
source = "mirror"
channel = "alpha"

[update.sources]
custom = "https://example.com/latest.json"
```

## 默认配置示例

起始配置文件位于 `crates/kuku/assets/default-config.toml`。
