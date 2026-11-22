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

首次运行、`kuku init` 和 TUI 中的 `/model` 共用 provider 设置。密钥存储和编辑行为见[配置指南](../start/configuration.md)。

终端客户端使用独立的偏好文件。[终端偏好](#终端偏好)

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
| `think` | `provider-default`\|`off`\|`low`\|`medium`\|`high` | 思考偏好。`provider-default` 省略请求中的 effort |
| `reasoning_effort` | string | 原生思考值，覆盖 `think` |
| `service_tier` | string | 独立于思考设置的原生服务选项 |
| `context_window` | integer | 最大输入 token 数 |
| `max_output_tokens` | 正整数，可省略 | 此 tier 的单次生成上限。供应商把推理 token 计入输出时，上限也包含推理 |
| `purpose` | string | 展示给用户和受委派 Agent 的路由用途 |
| `output` | `text`\|`image` | Agent 请求的输出类型，默认为文本 |

只配置一个路由即可使用。可通过 `default_model` 选择默认路由，单路由配置可省略此字段。已有 `strong`、`balanced` 和 `light` 路由仍可解析。TUI 中显式的 **Set current model as default** 操作会复用匹配的 tier；没有匹配项时才创建或覆盖一个普通的 `user-default` tier。`user-default` 没有特殊解析优先级，和其他 tier 一样由 `default_model` 选择。

Chat 和 Responses 省略 `max_output_tokens` 时使用上游默认值。Anthropic 原生 Messages 要求数值，内置 Sonnet 和 Haiku 预设在 TOML 中提供。自定义 Messages 连接可从 tier、Agent 或宿主获得所需数值。旧配置中显式设置的限额继续在对应 tier 生效。单次响应还受剩余上下文约束。

## `provider.<name>`

| Field | Type | Meaning |
|---|---|---|
| `format` | string | `anthropic`、`openai-chat` 或 `openai-responses` |
| `base_url` | string | 附加协议后缀的 provider API 基础 URL |
| `endpoint_url` | string | 完整请求 URL，按填写的地址发送请求 |
| `models_url` | string | 可选的标准模型发现完整 URL |
| `proxy` | table | 模型发现、试跑和正式请求共用的网络路径 |
| `model_pricing` | table | 按精确模型 ID 配置的美元计费规则 |
| `model_price_catalogs` | table | 模型设置保存的 OpenRouter 公开路由报价 |
| `model_capabilities` | table | 按模型 ID 记录列表声明、手工覆盖及试跑确认可传的思考档位 |
| `api_key` | string | `$ENV_VAR_NAME`、直接填写的 key，或 `file:<绝对路径>` |
| `auth` | `bearer`\|`x-api-key` | 独立于 `format` 覆盖认证方式 |
| `profile` | string | `auto`（默认）、`openai`、`anthropic`、`deepseek`、`openrouter` 或 `compatible` |
| `reasoning_efforts` | string[] | 覆盖该连接模型已声明的原生思考值 |
| `service_tiers` | string[] | 覆盖该连接模型已声明的原生服务值 |
| `routing` | table | OpenRouter 上游路由选项 |
| `prompt_cache_scope` | `workspace`\|`session`\|`off` | Responses 提示缓存键范围，默认为 `workspace` |

`api_key = "$NAME"` 只选择该环境变量。`api_key = "file:/absolute/path"` 只选择该文件；不接受相对文件引用，在请求时校验并读取文件，且两个来源之间不会自动回退。`file:` 是路径引用，不是 URL。`kuku config show` 只显示引用，不读取密钥。设置流程保存的密钥以未加密明文形式保存在 `<data-dir>/credentials/` 下的私有文件中；使用同一数据目录的工作区共享对应的凭据目录。Unix 权限和 Windows ACL 要求见[文件布局](file-layout.md)。已有密钥文件不会自动删除。

内置 `openai` 连接的 `base_url` 是 `https://api.openai.com/v1`，Responses 适配器会在末尾加上 `/responses`。使用网关时，填写该网关提供的 API 基础地址。[OpenAI Responses API](https://developers.openai.com/api/reference/resources/responses/methods/create)

连接通过 `base_url` 或 `endpoint_url` 设置地址。DeepSeek Chat 的基础地址 `https://api.deepseek.com` 和 `https://api.deepseek.com/v1` 都会在原路径上追加 `/chat/completions`。完整地址如 `https://gateway.example/gateway/infer` 会保留其路径。模型列表位于独立目录时，可将 `models_url` 设为 `https://gateway.example/catalog/models`。

`proxy.mode` 可设为 `environment`（默认）、`direct` 或 `explicit`。显式代理的 `proxy.url` 使用 `http://`、`socks5://` 或 `socks5h://`。SOCKS5 在本机解析域名，SOCKS5H 在代理端解析。需要认证时填写 `proxy.auth.username`，再指定 `proxy.auth.password_env` 或 `proxy.auth.password_file` 之一。密码文件必须使用绝对路径，与模型密钥文件执行同样的权限检查。

`model_pricing` 中的每条规则适用于表名对应的连接和精确模型。默认的 `automatic` 模式优先采用上游回执。当响应能唯一匹配实际端点和服务档时，也可以用模型设置保存的 OpenRouter 报价估算。重新运行模型设置可以刷新报价。`configured` 为直连的标准档配置手工费率。不同端点或服务档有独立费率时，用 `routed` 分别填写。上游提供完整端点标签时使用该标签，直连可省略。`usage-only` 保留 token 用量，费用记为未知。

费率单位为每百万 token 的美元单价。按需填写 `input`、`output`、`cache_read` 和 `cache_write`。供应商单独回报缓存写入时长时，可填写 `cache_write_5m` 和 `cache_write_1h`。估算记录会保留采用的费率及其来源。用量不完整或有多个可能的费率时，费用记为未知。

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

模型能力分别保存在 `discovered` 和 `manual` 下。字段包括 `reasoning_efforts`、`reasoning_default`、`input_modalities`、`output_modalities` 和 `interfaces`。`protocols` 可声明特定协议的字段，例如 `anthropic_messages.system_prompt_update`。解析结果标注来源，未声明的值保持未知，刷新时保留手工值。支持图像输入的路由可以提供[图像查看](tools.md)。模型声明支持图像输出或输出能力未知时，Agent 可以保存图像输出设置。内置适配器生成文本，并在发送图像输出请求前报告能力错误。[DeepSeek 模型列表](https://api-docs.deepseek.com/api/list-models/) · [OpenRouter 模型列表](https://openrouter.ai/docs/api/api-reference/models/list-all-models-and-their-properties)

`prompt_cache_scope` 只影响 Responses 请求。`workspace` 将规范化的绝对工作区身份与连接、端点和模型一起哈希，因此同一工作区中的 Agent 和 Session 可以共享缓存键，同时不会发送工作区路径。`session` 使用与传输关联相同的 Session、Agent 地址和连接身份派生独立键，并纳入端点和模型以隔离路由。`off` 不发送 `prompt_cache_key`。缺少关联上下文时，适配器不会生成缓存键。这些设置都不会改变传输关联或 OpenRouter 的 `session_id`。

SDK 按配置名称选择连接。多个连接可以使用同一个 `format`；单独选择协议不会选择第一条匹配连接。显式模型设置覆盖选中的 tier。选择其他连接不会携带无关默认 tier 的原生设置；显式选择的 tier 仍提供默认值，不兼容的选择会失败。

`auto` 根据已知端点主机名选择端点 profile。自定义网关使用 `compatible`，使用原生设置时必须显式声明能力。Profile 描述文档规定的适配行为，不代表上游已经确认。显式选择不支持的思考或服务值会在派发前失败。`think` 是可移植的 tier 偏好。所选模型和连接声明支持对应档位时，适配器才发送映射后的值；否则请求沿用上游默认行为，并记录原始偏好，有效档位留空。直连 DeepSeek 会把 `medium` 映射为 `high`。`reasoning_effort` 精确选择原生值，不会静默改档。启用服务 tier 不改变模型或思考设置。

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
| `threshold` | float | `0.95` |
| `keep_turns` | integer | `2` |

在 Session 中，`enabled` 控制自动上下文压缩。`threshold` 取值为 `0.0` 到 `1.0`，设置触发压缩时已用的可用输入 token 预算比例。运行时先为输出留出空间，再计算该比例。运行时检查可摘要的历史量，只在能腾出有效空间时提交后继段。手动 `/compact` 仍然可用。对于独立 SDK `Query`，`keep_turns` 决定模型生成交接后保留多少个此前轮次。

## `logs`

| Field | Type | Default |
|---|---|---|
| `max_age_days` | integer | `14` |
| `max_total_size_mb` | integer | `512` |

可观测性日志默认开启，且没有禁用开关。kuku 会先按时间清理日志，再按总大小预算清理。清理日志永远不会触碰 Session 的 `events.jsonl` 文件。Provider trace 另有[写入时上限](environment-variables.md#provider-trace-日志)。

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

## 终端偏好

终端启动时从本机客户端数据目录读取 `tui.toml`，对受管服务和显式远程连接都生效。目录按 `--data-dir`、`KUKU_HOME`、`~/.kuku` 的顺序选取。SDK 和服务的执行设置使用 `config.toml`。

`tui.toml` 或 `fields` 缺省时，使用内置顺序 `model,connection,agent,workspace,context,tps`。默认模型和连接字段显示已解析的模型 ID 和服务状态。显式选择 `model` 或 `connection` 时，空间足够也可显示路由详情或端点。`execution` 是可选字段。

从动作菜单打开 `/statusline` 即可在终端内编辑。用 ↑ 和 ↓ 选择字段，Space 勾选或取消，← 和 → 移动已勾选的字段。底栏会实时预览。R 预览内置布局，Enter 保存到 `tui.toml`，Esc 放弃修改。保存内置布局会移除 `statusline.fields`。

在 `tui.toml` 中添加以下内容可选择全部字段。

```toml
[statusline]
fields = [
  "model", "connection", "execution", "input", "agent", "workspace",
  "context", "tokens_in", "tokens_out", "cache", "cost", "tps",
]
```

按想要的顺序填写所需字段。用 `fields = []` 可隐藏状态栏。选择 `input` 会把输入反馈放在状态栏，默认位置为编辑区。[字段含义](cli.md#终端界面)

将 `KUKU_TUI_STATUSLINE` 设为逗号分隔的字段列表，可在本次启动时覆盖文件设置。`none` 隐藏状态栏，空白值选择内置字段。环境覆盖时，`/statusline` 仍可编辑并保存 `tui.toml` 的布局。关闭编辑面板后，环境变量继续生效。当前设置无效时，终端会显示错误并回退到内置字段。[环境变量](environment-variables.md)
