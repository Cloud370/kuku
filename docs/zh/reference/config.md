# Config

## Location

默认路径：

```text
~/.kuku/config.toml
```

如果设置了 `KUKU_HOME`，CLI 使用以下路径。

```text
$KUKU_HOME/config.toml
```

HTTP 服务默认读取所选数据目录下的 `config.toml`，也可通过 `--config` 指定路径。运行 `kuku server status` 可查看正在运行的受管服务的生效配置。[配置指南](../start/configuration.md)

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
| `reasoning_effort` | string | 原生思考值，覆盖 `think` |
| `service_tier` | string | 独立于思考设置的原生服务选项 |
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
| `auth` | `bearer`\|`x-api-key` | 独立于 `format` 覆盖认证方式 |
| `profile` | string | `auto`（默认）、`openai`、`anthropic`、`deepseek`、`openrouter` 或 `compatible` |
| `reasoning_efforts` | string[] | 覆盖该连接模型已声明的原生思考值 |
| `service_tiers` | string[] | 覆盖该连接模型已声明的原生服务值 |
| `routing` | table | OpenRouter 上游路由选项 |

内置 `openai` 连接的 `base_url` 是 `https://api.openai.com/v1`，Responses 适配器会在末尾加上 `/responses`。使用网关时，填写该网关提供的 API 基础地址。[OpenAI Responses API](https://developers.openai.com/api/reference/resources/responses/methods/create)

SDK 按配置名称选择连接。多个连接可以使用同一个 `format`；单独选择协议不会选择第一条匹配连接。显式模型设置覆盖选中的 tier。选择其他连接不会携带无关默认 tier 的原生设置；显式选择的 tier 仍提供默认值，不兼容的选择会失败。

`auto` 根据已知端点主机名选择端点 profile。自定义网关使用 `compatible`，使用原生设置时必须显式声明能力。Profile 描述文档规定的适配行为，不代表上游已经确认。显式选择不支持的思考或服务值会在派发前失败。`think` 保持为可移植的 tier 偏好：直接 DeepSeek 的 `medium` 映射为原生 `high`，请求值与有效值分别记录。`reasoning_effort` 精确选择原生值，不会静默改档。启用服务 tier 不改变模型或思考设置。

Messages 默认使用 `x-api-key` 认证；OpenRouter 三种协议都使用 Bearer。DeepSeek Messages 将 `thinking.type` 发送为 `enabled` 或 `disabled`，省略不支持的缓存控制并拒绝服务 tier。DeepSeek Responses 发送完整的无状态历史，绝不使用 `previous_response_id`。参见 [DeepSeek 思考](https://api-docs.deepseek.com/guides/thinking_mode/)、[Messages](https://api-docs.deepseek.com/guides/anthropic_api/) 和 [Responses](https://api-docs.deepseek.com/guides/responses_api/) 契约，以及 [OpenRouter Messages 认证](https://openrouter.ai/docs/api/api-reference/anthropic-messages/create-a-message)。

`routing.order` 和 `routing.only` 填写上游端点 tag。`allow_fallbacks` 控制上游回退；TOML 与 SDK 构造的 `require_parameters` 均默认 `true`。可选的 `routing.max_price.prompt` 和 `.completion` 限制每百万 token 的美元单价，不限制总支出。OpenRouter 收到由 Session、Agent 地址和连接名派生的稳定传输关联；不同 Agent 在跨 Turn 和重开后仍保持独立。路由亲和不保证缓存保留。参见[路由](https://openrouter.ai/docs/guides/routing/provider-selection)和[提示缓存](https://openrouter.ai/docs/guides/best-practices/prompt-caching)契约。

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

可观测性日志默认开启，且没有禁用开关。kuku 会先按时间清理日志，再按总大小预算清理。清理日志永远不会触碰 Session 的 `events.jsonl` 文件。`logs/provider-trace/` 下显式开启的 provider trace 文件也使用同一套保留预算。

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
