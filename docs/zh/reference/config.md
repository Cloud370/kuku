# 配置（Config）

## 配置文件位置

默认文件是数据目录下的 `config.toml`。数据目录按 `--data-dir`、`KUKU_HOME`、`~/.kuku` 的顺序选择。

服务可用 `--config PATH` 指定配置。运行 `kuku server status` 查看服务已加载的文件。首次配置和 TUI 的 `/model` 会编辑本地配置。[配置指南](../start/configuration.md) · [CLI 路径选择](cli.md#kuku-config) · [终端偏好](#终端偏好)

## 最小配置

```toml
default_model = { connection = "work", model = "your-model-id" }

[provider.work]
format = "openai-responses"
base_url = "https://gateway.example/v1"
api_key = "$MY_GATEWAY_KEY"
```

用该连接实际提供的 URL 和模型 ID 替换示例值。在服务的环境中设置 `MY_GATEWAY_KEY`，也可改为自己选的变量名。必填的 `default_model` 已注册该连接上的模型，因此这份配置可以直接使用。[环境变量引用](environment-variables.md#配置中的引用)

在 TUI 或通过 CLI 补丁切换默认模型时，只要之前的连接仍在且没有显式删除旧模型，kuku 会保留那条模型注册。手动编辑配置时，可以先加上 `[model.old_connection."old-model"]` 这样的表，再改 `default_model`，这样已有 Session 仍能使用原模型。删除模型后，依赖它的 Agent 需要切换到其他已配置模型。

CLI 补丁编辑本地服务已加载的文件版本时，kuku 以服务实际解析出的原默认模型为准。如果编辑进程中的旧模型变量取值不同或尚未设置，kuku 会使用精确 ID 为原先隐式注册的模型建表。

## 顶层配置项

| 键 | 类型 | 含义 |
|---|---|---|
| `default_model` | table | 必填的新 Session 默认 `connection` 和精确 `model` ID |
| `model.<connection>."<model-id>"` | table | 一个已注册模型的设置 |
| `provider.<name>` | table | Provider 连接定义 |
| `discovery` | table | Agent 与 Skill 自动发现设置 |
| `handoff` | table | 上下文压缩（Handoff）设置 |
| `logs` | table | 可观测性日志保留策略 |
| `plugin` | table | Package 扩展加载开关 |
| `update` | table | 更新源与频道 |

## 模型设置

要添加模型或设置默认模型的选项，按连接和模型分别添加表。模型 ID 含点、斜杠等 TOML 键的特殊字符时须加引号。同一个 ID 可以在不同连接上注册，并分别设置。

```toml
[model.work."your-model-id"]
context_window = 200000
max_output_tokens = 8192

[model.work."another-model-id"]
max_output_tokens = 4096
```

| 字段 | 类型 | 含义 |
|---|---|---|
| `think` | `provider-default`\|`off`\|`low`\|`medium`\|`high` | 可移植思考偏好，默认 `provider-default` |
| `reasoning_effort` | string | 协议原生推理值，覆盖 `think` |
| `service_tier` | string | 协议原生服务档位，独立于思考设置 |
| `context_window` | integer | 最大输入 token 数，默认 200,000 |
| `max_output_tokens` | 正整数（可选） | 生成上限（若 Provider 计入输出，也含思考 token） |
| `purpose` | string | 展示给用户和子 Agent 的路由用途说明 |
| `output` | `text`\|`image` | 期望输出类型，默认 `text` |
| `request` | table | 模型路由的[请求头与请求体](#请求头与请求体)覆盖项 |

Chat 与 Responses 省略 `max_output_tokens` 时沿用上游默认值。原生 Anthropic Messages 需要从模型设置、Agent 或宿主取得数值。每次响应仍受可用上下文余量约束。

## `provider.<name>`

| 字段 | 类型 | 含义 |
|---|---|---|
| `format` | string | `anthropic`、`openai-chat` 或 `openai-responses` |
| `base_url` | string | API 基础地址（会自动追加协议后缀） |
| `endpoint_url` | string | 完整请求地址，设置后原样使用 |
| `models_url` | string | 可选的独立模型列表地址 |
| `proxy` | table | 发现、探测与模型请求的网络代理设置 |
| `model_pricing` | table | 按精确模型 ID 配置的美元计费规则 |
| `model_price_catalogs` | table | 配置流程保存的 OpenRouter 公开报价 |
| `model_capabilities` | table | 按模型 ID 记录列表声明、手工覆盖、试跑记录中的思考档位和自定义服务档位 |
| `api_key` | string | `$ENV_VAR_NAME`、明文密钥或 `file:<绝对路径>` |
| `subscription` | table | 订阅账号引用，包含 `channel` 和绝对路径 `credential_file` |
| `auth` | `bearer`\|`x-api-key` | 独立于 `format` 覆盖认证头方式 |
| `profile` | string | `auto`（默认）、`openai`、`anthropic`、`deepseek`、`openrouter` 或 `compatible` |
| `reasoning_efforts` | string[] | 覆盖该连接声明的原生推理取值列表 |
| `service_tiers` | string[] | 覆盖该连接声明的原生服务档位列表 |
| `routing` | table | OpenRouter 上游路由选项 |
| `prompt_cache_scope` | `workspace`\|`session`\|`off` | Responses Prompt 缓存键范围，默认 `workspace` |
| `request` | table | 连接级[请求头与请求体](#请求头与请求体)配置 |

### 端点与代理

- `base_url` 与 `endpoint_url` 二选一。Responses 会在 `https://api.openai.com/v1` 这样的基础地址后追加 `/responses`，Chat Completions 会追加 `/chat/completions`。适配器直接使用完整的 `endpoint_url`。若模型目录在独立地址，设置 `models_url`。[OpenAI Responses API](https://developers.openai.com/api/reference/resources/responses/methods/create)
- `proxy.mode` 支持 `environment`（默认）、`direct` 或 `explicit`。`explicit` 需填写 `proxy.url`（`http://`、客户端解析 DNS 的 `socks5://` 或代理解析 DNS 的 `socks5h://`）。带认证的代理设置 `proxy.auth.username` 以及 `proxy.auth.password_env` 或 `proxy.auth.password_file`（绝对路径，沿用密钥文件权限校验）。

### 费率与费用估算

- `model_pricing.<model>.mode` 默认为 `automatic`：优先使用 Provider 返回的计费回执；若响应匹配单一 OpenRouter 端点与服务档位，则用配置时保存的公开报价估算（重新配置会刷新报价）。
- `configured` 为直连标准档位设置人工费率（美元/百万 token，支持 `input`、`output`、`cache_read`、`cache_write` 以及分时长的 `cache_write_5m` / `cache_write_1h`）。
- `routed` 为不同端点标签或 `service_tier` 分别配置多条 `routes` 费率。
- `usage-only` 只记 token 不算费用。缺少用量或存在多个候选价格时金额同样记为未知。

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

### 模型能力、Profile 与路由

保存模型配置时，成功的图像输入测试只对测试时使用的端点、协议和精确模型有效。显式手工声明仍然优先。Responses 路由通过图像输入验证，或在 `manual.protocols.responses.input_modalities` 中显式声明支持图像后，可接收用户附件，但这不会启用工具结果中的图像。CLI 的 `config test` 只报告探测证据，不改动配置。

每个模型能力条目也可以包含 `custom_service_tiers` 表。键为原生档位名，值为 `unverified`（由用户声明）或 `confirmed`（探测回报了请求的档位）。这些值会补充模型目录或手动覆盖中的档位列表。例如 `provider.gateway.model_capabilities."example/model".custom_service_tiers.turbo = "unverified"` 为该连接和指定模型添加 `turbo`。

- **能力声明**：`model_capabilities.<model>` 区分 `discovered` 与 `manual`（支持 `reasoning_efforts`、`reasoning_default`、`input_modalities`、`output_modalities`、`interfaces` 及协议级扩展如 `protocols.anthropic_messages.system_prompt_update`）。刷新发现时保留手工覆盖，未填字段保持未知。声明图像输入的路由可启用[图像检查工具](tools.md)；内置文本适配器会在派发前拒绝图像输出请求。[DeepSeek 模型列表](https://api-docs.deepseek.com/api/list-models/) · [OpenRouter 模型列表](https://openrouter.ai/docs/api/api-reference/models/list-all-models-and-their-properties)
- **Profile 与思考等级**：`profile = "auto"` 按已知主机名选择配置，自定义网关走 `compatible` 且原生选项需显式声明能力。连接严格按名称匹配，不会只凭 `format` 选第一个连接。`think` 在模型声明支持时映射并下发，否则交由上游默认（DeepSeek 直连将 `medium` 映射为 `high`）；`reasoning_effort` 按精确原生值下发，绝不静默改写，不支持时在派发前报错。
- **厂商细节**：Messages 默认用 `x-api-key`；OpenRouter 对三种协议统一使用 Bearer 认证。DeepSeek Messages 发送 `thinking.type`（`enabled`/`disabled`），省略不支持的缓存控制并拒绝 service tier；DeepSeek Responses 保持无状态完整历史，不使用 `previous_response_id`。[DeepSeek 思考模式](https://api-docs.deepseek.com/guides/thinking_mode/) · [DeepSeek Messages](https://api-docs.deepseek.com/guides/anthropic_api/) · [DeepSeek Responses](https://api-docs.deepseek.com/guides/responses_api/) · [OpenRouter Messages 认证](https://openrouter.ai/docs/api/api-reference/anthropic-messages/create-a-message)
- **OpenRouter 路由**：`routing.order` / `routing.only` 指定端点标签；`allow_fallbacks` 控制回退；`require_parameters` 默认 `true`；`routing.max_price.prompt` / `.completion` 限制百万 token 单价（非总花费上限）。OpenRouter 会收到按 Session、Agent 与连接名派生的稳定关联标识（亲和性不保证上游缓存留存）。[OpenRouter 路由](https://openrouter.ai/docs/guides/routing/provider-selection) · [Prompt 缓存](https://openrouter.ai/docs/guides/best-practices/prompt-caching)

### 请求头与请求体

`request.headers` 将 HTTP 请求头名称映射到字符串。`request.body` 将顶层 JSON 字段映射到 TOML 值，支持数组和表。这些设置作用于内置推理请求，模型试探也使用连接级设置。

内置 TOML 预设、连接配置和匹配的模型路由配置依次生效。请求头名称不区分大小写。请求体按顶层字段替换整个值，包括 OpenRouter 的 `provider` 这类嵌套对象。在任一级设置 `request.inherit = false`，可以从空的请求字段开始配置。

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

将这些表添加到已有的 OpenRouter 连接和模型路由。`request.body.provider` 优先于连接的 `routing` 表，原生路由和数据策略字段由 OpenRouter 定义。 [OpenRouter 路由文档](https://openrouter.ai/docs/guides/routing/provider-selection)

- `"{{session_id}}"` 展开为同一 Session、Agent 和连接内稳定的匿名标识。`"{{prompt_cache_key}}"` 使用 `prompt_cache_scope` 选定的键。无法取得值时省略对应字段。
- `"{{request_id}}"` 仅用于请求头，每次 HTTP 请求尝试生成新值。占位符替换整段字符串，也可用于请求体对象内的字符串。
- 请求头可以引用 `"$NAME"`。服务在准备请求时读取变量，Provider 请求 trace 会对配置的请求头值和自定义请求体字段脱敏。请求体字符串按字面值处理，支持上述占位符。
- 凭据和 HTTP 帧相关请求头、模型和消息内容、工具定义、流式与有状态续接字段，以及已解析的思考、服务档位和输出上限由适配器管理。配置这些保留字段时会报错。
- ChatGPT 登录会在发送前拒绝显式配置的预览版不支持字段，例如 `metadata`、`temperature` 和 `prompt_cache_retention`。 [订阅请求限制](https://developers.openai.com/siwc/token-sharing-open-source/preview-limitations)

### Prompt 缓存与凭据

| 渠道 | 请求默认值与缓存规则 |
|---|---|
| ChatGPT 订阅 | 实际请求发送稳定的 `session-id` 来帮助查找缓存，每次 HTTP 请求生成新的 `X-Client-Request-Id`。只有连接或模型路由显式添加时，请求才会带 `prompt_cache_key` 或 `x-session-id`。 [Codex 会话路由](https://github.com/openai/codex/pull/44862) |
| OpenAI API Key Responses | 自动使用前缀缓存。GPT-5.6 之前的模型在缓存键可用时发送 `prompt_cache_key` 帮助路由。GPT-5.6 及更新模型可按需用它区分缓存用量。 [OpenAI Prompt 缓存](https://developers.openai.com/api/docs/guides/prompt-caching) |
| OpenRouter | 请求体中的 `session_id` 有助于稳定选中的上游。缓存规则取决于实际上游和模型。Claude 路由还会收到顶层 `cache_control`。 [OpenRouter Prompt 缓存](https://openrouter.ai/docs/guides/best-practices/prompt-caching) |
| 原生 Anthropic Messages | 顶层 `cache_control = { type = "ephemeral" }` 启用自动缓存，缓存断点随会话增长向后移动。 [Anthropic Prompt 缓存](https://platform.claude.com/docs/en/build-with-claude/prompt-caching) |

命中需要前缀匹配，且请求能找到有效缓存。续聊时保持模型、工具定义、系统提示和已有消息稳定，在末尾追加新输入。首次请求、缓存过期、前缀变化或服务路由变化都有可能返回零。即使 Session 和前缀相同，正常完成的续轮也可能偶尔读到零。 [OpenAI Prompt 缓存](https://developers.openai.com/api/docs/guides/prompt-caching) · [Codex 缓存复现](https://github.com/openai/codex/issues/44716)

Responses 从 `usage.input_tokens_details.cached_tokens` 读取缓存用量，Chat Completions 使用 `usage.prompt_tokens_details.cached_tokens`，Anthropic Messages 使用 `usage.cache_read_input_tokens`。Messages 通过 `usage.cache_creation_input_tokens` 单独报告新写入的 token 数。上游省略用量字段时，读取量记为未知。

Anthropic 默认缓存 5 分钟。Haiku 5.5 的 5 分钟缓存写入价为普通输入的 1.25 倍，1 小时为 2 倍。两档缓存读取价均为普通输入的 0.1 倍。 [Anthropic Prompt 缓存价格](https://platform.claude.com/docs/en/build-with-claude/prompt-caching)

模型试探和网页摘要等一次性请求只在用户显式配置时发送 `cache_control`。输出截断后的恢复请求沿用原会话标识。无状态的 OpenAI Responses 由上游默认返回加密推理内容。 [OpenAI 无状态推理](https://developers.openai.com/api/docs/guides/reasoning#preserve-reasoning-without-stored-responses)

- **`prompt_cache_scope`**（仅 Responses）控制 `{{prompt_cache_key}}` 占位符。`workspace`（默认）对规范化的工作区路径、连接、端点和模型做哈希。`session` 对 Session、Agent、连接、端点和模型做哈希。`off` 时省略引用该占位符的字段。
- **`api_key`**：`$NAME` 仅读环境变量；`file:/绝对路径` 在请求时校验并读取文件（不接受相对路径、非 URL 语法、两类来源不互退）。交互式配置将明文密钥写入 `<data-dir>/credentials/`，`kuku config show` 只展示引用。[文件布局](file-layout.md)
- `$NAME` 的解析时机见[环境变量](environment-variables.md)。

订阅连接通过账号引用读取凭据。ChatGPT 订阅使用 OpenAI 官方 Responses 和模型目录地址。账号认证也沿用该连接的网络模式。手动设置 SOCKS 代理时，使用 `socks5h://` 由代理解析地址。[订阅登录](../start/configuration.md#在无头主机登录订阅)

## `discovery`

| 字段 | 类型 | 默认值 |
|---|---|---|
| `auto_discover` | bool | `true` |
| `extra_user_paths` | string[] | `[]` |
| `extra_project_paths` | string[] | `[]` |

`auto_discover` 扫描常见用户级与项目级点目录下的 `skills`、`agents` 和 `agent` 子目录。

## `handoff`

| 字段 | 类型 | 默认值 |
|---|---|---|
| `enabled` | bool | `true` |
| `threshold` | float | `0.95` |
| `keep_turns` | integer | `2` |

当已使用的输入 token 达到可用预算的 `threshold` 比例时，开始自动压缩。取值范围为 `0.0` 到 `1.0`。计算比例时会计入推理开销，并为输出预留有界的空间。模型的生成上限独立生效。摘要腾出足够空间时，运行时将提交新的上下文段。自动压缩失败后，运行时等有更多材料可压缩再重试，也可以手动运行 `/compact`。独立 SDK `Query` 用 `keep_turns` 设置交接后保留的最近轮数。

## `logs`

| 字段 | 类型 | 默认值 |
|---|---|---|
| `max_age_days` | integer | `14` |
| `max_total_size_mb` | integer | `512` |

可观测性日志默认常开，先按天数再按总大小清理，绝不触碰 Session 的 `events.jsonl`。[Provider Trace 上限](environment-variables.md#provider-trace-日志)

## `plugin`

| 字段 | 类型 | 默认值 |
|---|---|---|
| `enabled` | bool | 新默认配置为 `true` |

控制是否从 `.kuku/packages/` 加载 Package 提供的 Hook 与 Skill。

## `update`

| 字段 | 类型 | 默认值 |
|---|---|---|
| `source` | string | `github` |
| `channel` | string | `stable` |
| `sources` | table | 空 |

`source` 支持 `github`（内置发布清单）或 `mirror`（自定义镜像地址）；`channel` 支持 `stable` 或 `alpha`。

```toml
[update]
source = "mirror"
channel = "alpha"

[update.sources]
custom = "https://example.com/latest.json"
```

## 默认配置示例

初始模板包含可选的运行时设置和注释中的连接示例。[初始配置模板](../../../crates/kuku/assets/default-config.toml)

## 终端偏好

TUI 在本地和远程连接下均从本地客户端数据目录（`--data-dir` → `KUKU_HOME` → `~/.kuku`）读取 `tui.toml`；执行相关配置仍在 `config.toml` 中。

未设置 `tui.toml` 或 `statusline.fields` 时，默认顺序为 `model,agent,workspace,context,cache,tps,tokens_in,tokens_out,cost`。显式列出 `model` 可在空间允许时显示路由。可选字段 `connection` 也会在连接正常时显示本地或远程连接状态。`execution` 为可选字段。

可选字段 `permission_mode` 显示当前工作区的权限模式。

在 TUI 中运行 `/statusline` 可用 `Space` 开关、`←`/`→` 排序、`R` 预览默认并按 `Enter` 保存到 `tui.toml`（保存默认布局会移除 `statusline.fields`）。

全部可用字段示例：

```toml
[statusline]
fields = [
  "model", "connection", "execution", "input", "agent", "workspace",
  "context", "tokens_in", "tokens_out", "cache", "cost", "tps", "permission_mode",
]
```

设 `fields = []` 可隐藏状态栏；加入 `input` 会把输入状态反馈从输入框移至状态栏。[字段含义](cli.md#终端界面)

设置 `KUKU_TUI_STATUSLINE` 为逗号分隔列表可做单次覆盖（`none` 隐藏，空值或无效值回退默认布局）。[环境变量](environment-variables.md)

用 `/notifications` 打开声音设置。↑ 和 ↓ 选择，空格切换，Enter 保存。`tui.toml` 的 `[notifications]` 下有 `question`、`permission`、`interrupted` 和 `completed` 四个开关，默认都为 `true`。TUI 对新事件发送 BEL，由终端决定响铃、闪屏或静音。历史记录和重连回放保持安静。
