# 配置（Config）

## 配置文件位置

默认路径为 `~/.kuku/config.toml`；设置 `KUKU_HOME` 后为 `$KUKU_HOME/config.toml`。

HTTP 服务默认读取数据目录下的 `config.toml`，也可通过 `--config <path>` 指定。运行中的受管服务可用 `kuku server status` 查看实际生效配置。

首次运行向导、`kuku init` 与 TUI `/model` 均读写此文件。[配置指南](../start/configuration.md) · [终端偏好](#终端偏好)

## 顶层配置项

| 键 | 类型 | 含义 |
|---|---|---|
| `default_model` | string | 默认模型档位名 |
| `model.<name>` | table | 模型档位定义 |
| `provider.<name>` | table | Provider 连接定义 |
| `discovery` | table | Agent 与 Skill 自动发现设置 |
| `handoff` | table | 上下文压缩（Handoff）设置 |
| `logs` | table | 可观测性日志保留策略 |
| `plugin` | table | Package 扩展加载开关 |
| `update` | table | 更新源与频道 |

## `model.<name>`

| 字段 | 类型 | 含义 |
|---|---|---|
| `provider` | string | 对应 `provider.<name>` 中的连接名 |
| `model` | string | Provider 模型 ID |
| `think` | `provider-default`\|`off`\|`low`\|`medium`\|`high` | 可移植思考偏好（`provider-default` 不下发线级 effort） |
| `reasoning_effort` | string | 协议原生推理值，覆盖 `think` |
| `service_tier` | string | 协议原生服务档位，独立于思考设置 |
| `context_window` | integer | 最大输入 token 数 |
| `max_output_tokens` | 正整数（可选） | 本档位生成上限（若 Provider 计入输出，也含思考 token） |
| `purpose` | string | 展示给用户和子 Agent 的路由用途说明 |
| `output` | `text`\|`image` | 期望输出类型，默认 `text` |

可通过 `default_model` 指定默认档位；只有一个路由时也可省略。既有 `strong`、`balanced`、`light` 路由继续有效。TUI 的 **Set current model as default** 会复用匹配档位，或创建/更新普通 `user-default` 档位并通过 `default_model` 选中。

Chat 与 Responses 省略 `max_output_tokens` 时沿用上游默认值；原生 Anthropic Messages 需要数值（内置 Sonnet/Haiku 预设已在 TOML 中填写，自定义 Messages 路由可从档位、Agent 或宿主获取）。单次响应仍受可用上下文余量约束。

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
| `model_capabilities` | table | 按精确模型 ID 配置的能力发现与手工覆盖 |
| `api_key` | string | `$ENV_VAR_NAME`、明文密钥或 `file:<绝对路径>` |
| `subscription` | table | 订阅账号引用，包含 `channel` 和绝对路径 `credential_file` |
| `auth` | `bearer`\|`x-api-key` | 独立于 `format` 覆盖认证头方式 |
| `profile` | string | `auto`（默认）、`openai`、`anthropic`、`deepseek`、`openrouter` 或 `compatible` |
| `reasoning_efforts` | string[] | 覆盖该连接声明的原生推理取值列表 |
| `service_tiers` | string[] | 覆盖该连接声明的原生服务档位列表 |
| `routing` | table | OpenRouter 上游路由选项 |
| `prompt_cache_scope` | `workspace`\|`session`\|`off` | Responses Prompt 缓存键范围，默认 `workspace` |

### 端点与代理

- `base_url` 与 `endpoint_url` 二选一。内置 `openai` 使用 `https://api.openai.com/v1`（Responses 追加 `/responses`；DeepSeek Chat 会在 `https://api.deepseek.com` 或 `/v1` 后追加 `/chat/completions`）。`endpoint_url` 原样保留路径；若模型目录在独立地址，设置 `models_url`。[OpenAI Responses API](https://developers.openai.com/api/reference/resources/responses/methods/create)
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

- **能力声明**：`model_capabilities.<model>` 区分 `discovered` 与 `manual`（支持 `reasoning_efforts`、`reasoning_default`、`input_modalities`、`output_modalities`、`interfaces` 及协议级扩展如 `protocols.anthropic_messages.system_prompt_update`）。刷新发现时保留手工覆盖，未填字段保持未知。声明图像输入的路由可启用[图像检查工具](tools.md)；内置文本适配器会在派发前拒绝图像输出请求。[DeepSeek 模型列表](https://api-docs.deepseek.com/api/list-models/) · [OpenRouter 模型列表](https://openrouter.ai/docs/api/api-reference/models/list-all-models-and-their-properties)
- **Profile 与思考等级**：`profile = "auto"` 按已知主机名选择配置，自定义网关走 `compatible` 且原生选项需显式声明能力。连接严格按名称匹配，不会只凭 `format` 选第一个连接。`think` 在模型声明支持时映射并下发，否则交由上游默认（DeepSeek 直连将 `medium` 映射为 `high`）；`reasoning_effort` 按精确原生值下发，绝不静默改写，不支持时在派发前报错。
- **厂商细节**：Messages 默认用 `x-api-key`；OpenRouter 对三种协议统一使用 Bearer 认证。DeepSeek Messages 发送 `thinking.type`（`enabled`/`disabled`），省略不支持的缓存控制并拒绝 service tier；DeepSeek Responses 保持无状态完整历史，不使用 `previous_response_id`。[DeepSeek 思考模式](https://api-docs.deepseek.com/guides/thinking_mode/) · [DeepSeek Messages](https://api-docs.deepseek.com/guides/anthropic_api/) · [DeepSeek Responses](https://api-docs.deepseek.com/guides/responses_api/) · [OpenRouter Messages 认证](https://openrouter.ai/docs/api/api-reference/anthropic-messages/create-a-message)
- **OpenRouter 路由**：`routing.order` / `routing.only` 指定端点标签；`allow_fallbacks` 控制回退；`require_parameters` 默认 `true`；`routing.max_price.prompt` / `.completion` 限制百万 token 单价（非总花费上限）。OpenRouter 会收到按 Session、Agent 与连接名派生的稳定关联标识（亲和性不保证上游缓存留存）。[OpenRouter 路由](https://openrouter.ai/docs/guides/routing/provider-selection) · [Prompt 缓存](https://openrouter.ai/docs/guides/best-practices/prompt-caching)

### Prompt 缓存与凭据

- **`prompt_cache_scope`**（仅 Responses）：`workspace`（默认）对规范化工作区路径+连接+端点+模型做哈希，同工作区多 Session 可共享缓存键且不外泄路径；`session` 按 Session+Agent+连接+端点+模型隔离；`off` 不发 `prompt_cache_key`。不影响 OpenRouter `session_id`。
- **`api_key`**：`$NAME` 仅读环境变量；`file:/绝对路径` 在请求时校验并读取文件（不接受相对路径、非 URL 语法、两类来源不互退）。交互式配置将明文密钥写入 `<data-dir>/credentials/`，`kuku config show` 只展示引用。[文件布局](file-layout.md)
- 配置中其余以 `$` 开头的字符串值均会在校验前展开环境变量。

订阅连接通过账号引用读取凭据。ChatGPT 订阅使用 OpenAI 官方 Responses 和模型目录地址，要求显式配置 HTTP 或 SOCKS5H 代理。账号认证请求也使用该连接的代理。[订阅登录](../start/configuration.md#在无头主机登录订阅)

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

初始配置模板见 `crates/kuku/assets/default-config.toml`。

## 终端偏好

TUI 在本地和远程连接下均从本地客户端数据目录（`--data-dir` → `KUKU_HOME` → `~/.kuku`）读取 `tui.toml`；执行相关配置仍在 `config.toml` 中。

未设置 `tui.toml` 或 `statusline.fields` 时，默认顺序为 `model,agent,workspace,context,cache,tps,tokens_in,tokens_out,cost`。显式列出 `model` 可在空间允许时显示路由。可选字段 `connection` 也会在连接正常时显示本地或远程连接状态。`execution` 为可选字段。

在 TUI 中运行 `/statusline` 可用 `Space` 开关、`←`/`→` 排序、`R` 预览默认并按 `Enter` 保存到 `tui.toml`（保存默认布局会移除 `statusline.fields`）。

全部可用字段示例：

```toml
[statusline]
fields = [
  "model", "connection", "execution", "input", "agent", "workspace",
  "context", "tokens_in", "tokens_out", "cache", "cost", "tps",
]
```

设 `fields = []` 可隐藏状态栏；加入 `input` 会把输入状态反馈从输入框移至状态栏。[字段含义](cli.md#终端界面)

设置 `KUKU_TUI_STATUSLINE` 为逗号分隔列表可做单次覆盖（`none` 隐藏，空值或无效值回退默认布局）。[环境变量](environment-variables.md)
