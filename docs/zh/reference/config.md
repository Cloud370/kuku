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
| `plugin` | table | package hook 执行开关 |
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

任何首字符为 `$` 的字符串类型的配置值会被视作环境变量引用并在校验前解析，唯一的例外是 `api_key`，其保留环境变量引用并延迟到使用该 provider 时才解析。

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

当估算的上下文使用量超过 `threshold` 时，kuku 会写入 handoff 摘要，并只在活动历史中保留最近的 `keep_turns` 轮。

## `plugin`

| Field | Type | Default |
|---|---|---|
| `enabled` | bool | 新默认配置中为 `true` |

这个开关控制 `.kuku/packages/` 中 package 的 hook 执行。

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

规范的起始文件位于 `crates/kuku/assets/default-config.toml`。
