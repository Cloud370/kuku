# HTTP 服务接口

终端、脚本和浏览器客户端共用 HTTP Session 契约。服务负责执行，并在服务端机器上解析工作区、模型选择和模型凭据。观察客户端断开后，服务可以继续执行，客户端随后仍可重连。关于本地服务发现与显式启动方式，见[运行服务](../guides/run-the-server.md)。

## 认证与版本

客户端访问接口时发送 `Authorization: Bearer TOKEN`，回环连接也需要认证。`kuku web` 打开受管本地服务的内嵌界面时，通过 URL 片段传递短时一次性票据。页面清除片段，将票据交换成独立的浏览器令牌，并保存在该标签页的 `sessionStorage`。刷新同一标签页时可继续用该令牌连接同一服务实例。显式启动的服务使用其配置的访问令牌。服务的默认工具策略信任已认证的用户，允许文件和命令工具。

公开的 `GET /health` 返回 `version`、`protocol_version` 和 `management_version`。`protocol_version` 描述 Session API。升级受管服务时，即使 Session API 版本不同，仍可以查看服务状态和请求正常重启。缺少令牌或令牌无效时，返回 `401` 和 `auth_required`。受管服务还会检查回环主机和请求携带的浏览器来源。

## 接口一览

| 方法 | 路径 | 用途 |
|---|---|---|
| `GET` | `/health` | 构建与协议版本 |
| `GET` | `/service/status` | 生效的服务身份与活动 |
| `GET` | `/service/model-default-plan` | 根据受管服务配置验证仅本地写入的模型默认计划 |
| `POST` | `/service/restart` | 请求受管服务释放 Session 后退出，以便正常替换 |
| `POST` | `/service/browser-ticket` | 认证后签发一次性浏览器接入票据 |
| `POST` | `/service/browser-session` | 在受管浏览器来源交换票据 |
| `GET` | `/sessions` | 列出保留的 Session，可按 `workspace` 或 `search` 过滤 |
| `DELETE` | `/sessions/{id}` | 删除已关闭的 Session |
| `POST` | `/sessions/{id}/commands` | 提交持久命令 |
| `GET` | `/sessions/{id}/receipts/{command_id}` | 查找已提交的命令回执 |
| `GET` | `/sessions/{id}/agents` | 查看工作区内该 Session 的 Agent 摘要 |
| `GET` | `/sessions/{id}/snapshot` | 与已提交游标对应的 Agent 和队列状态 |
| `GET` | `/sessions/{id}/history` | 读取游标之前的对话条目 |
| `GET` | `/sessions/{id}/history/rewinds/{rewind_event_id}` | 只读查看一次已提交的消息回退，不改变活动历史 |
| `GET` | `/sessions/{id}/observe` | 观察游标之后的已提交条目和临时进度 |
| `GET` | `/sessions/{id}/events` | 分页读取原始已提交事实 |
| `GET` | `/sessions/{id}/outputs/{output_id}` | 有界读取保留的工具输出 |
| `GET` | `/sessions/{id}/jobs` | 列出所选 Agent 的后台 Job |
| `GET` | `/sessions/{id}/jobs/{job_id}` | 查看 Job 及可用的结果 |
| `DELETE` | `/sessions/{id}/jobs/{job_id}` | 取消 Job 并返回其终态 |
| `GET` | `/sessions/{id}/values` | 列出所选 Agent 的命名值 |
| `PUT` | `/sessions/{id}/values/{name}` | 绑定命名值或显式替换来源 |
| `GET` | `/models` | 已配置的模型目录 |
| `GET` | `/models/resolve` | 解析候选 Agent 配置的生效设置 |
| `GET` | `/workspaces` | 列出工作区 |
| `GET` | `/workspaces/resolve`、`/workspaces/files`、`/workspaces/skills` | 解析服务端路径，或查找工作区文件与 Skill |

状态响应包含构建和协议版本、`instance_id`、`pid`、生效的 `data_dir` 和 `config_path`、`draining` 以及 `busy`。受管服务重启返回 `202` 表示已释放 writer，进程正在停止。`409` 和 `service_busy` 在 `busy` 列出正在进行的工作，包括运行中的 Job。受管客户端会等旧进程释放服务租约后启动替代进程。

`GET /service/model-default-plan` 仅适用于存在活动配置文件的受管服务。它要求常规 Bearer 认证，并执行相同的回环主机和来源检查，响应包含 `Cache-Control: no-store`，只接受 `connection`、`model` 和 `config_sha256` 查询字段。服务在自身进程环境中读取活动配置并验证请求路由；`config_sha256` 必须与客户端持有的原始字节一致。响应只包含 `tier`、`overwrite_user_default` 和 `config_sha256`，不会写入文件、解析凭据或发送模型请求。客户端仍负责本地且带并发保护的配置编辑。配置已变化、路由不可用、服务非受管或配置不可读时，会返回不泄露敏感信息的通用错误。

## 命令与回执

首次发送前选定唯一的 Session ID 和命令 ID。请求体包含服务端工作区的绝对路径、目标 Agent 地址和命令标识。向新 Session 发送时，在 `create` 中写入 Agent 初始配置。

```json
{
  "workspace": "/code/my-project",
  "agent": "main",
  "command_id": "input-389cd599",
  "type": "send",
  "text": "Summarize this directory",
  "timing": "steer",
  "create": {}
}
```

`create: {}` 使用已配置的默认值。初次发送时在 `create` 中指定模型设置，已有 Agent 可使用显式 `configure` 命令。模型选择由 [SDK](sdk.md#操作) 解析。Agent 的上下文和设置会跨客户端保留。

| 命令 `type` | 其他字段 | 行为 |
|---|---|---|
| `send` | `text`，可选 `timing`、`create` 和 `values` | 持久投递。默认的 `steer` 可在下一个安全的模型请求边界接纳，`next_turn` 等待下一轮。 |
| `create_agent` | `config` | 按地址创建拥有独立上下文的 Agent。 |
| `configure` | `config` | 在安全边界修改模型或默认值。 |
| `reply_question` | `request_id`、`text` | 回答精确的待决问题。 |
| `reply_permission` | `request_id`、`allow` | 决定精确的待决授权。 |
| `withdraw` | `input_command_id` | 撤回排队输入或返回其当前状态。 |
| `replace_queued` | `input_command_id`、`text` | 在接纳前替换文本，保留队列位置和投递时机。 |
| `compact` | | 压缩所选空闲或已中断的 Agent 的较早上下文。通过 `compaction` 历史条目观察进度和结果。重试失败的压缩时使用新的命令 ID。 |
| `restore_context` | | 撤销所选 Agent 最近一次仍生效的压缩，保留可见消息。 |
| `advance` | | 显式继续 Agent 的工作。 |
| `cancel` | `execution_id` | 取消观察到的那次执行并等待清理。 |

命令成功时返回含 `ok`、`receipt` 和 `replayed` 的 JSON。回执包含 `command_id`、`event_id` 和 `agent`。输入命令可以报告 `status` 为 `queued`、`accepted` 或 `withdrawn`。持久回执表示已经投递。模型接纳和执行结算分别记录。

在给已有 Agent 发送输入的同时修改模型选择时，传入 `config_patch`。若脚本需要在已有 Session 中创建缺失的 Agent，还要传入 `create_agent_if_missing: true`。新 Session 使用 `create` 设置初始 Agent，也可以用 `prompts_dir` 指定服务端自定义 Prompt 资源的绝对路径。

通过 `send` 的 `bootstrap_skill` 为该条输入指定 Skill，在接纳时应用。

`replace_queued` 在同一次提交中记录新回执，并将原输入标为已撤回。若原输入已经接纳或因其他原因离开队列，命令返回 `409` 和 `input_not_queued`。

如果没有收到响应，带上 `workspace` 查询回执接口。缺失的回执返回 HTTP `404` 和 `receipt_not_found`。要重发时，保持原 Session、Agent、命令标识和请求内容。重放已提交命令会返回原回执。以相同的命令标识提交冲突内容会返回 HTTP `409`。`?replay_only=true` 用于校验重放请求，命令尚未提交时返回 `receipt_not_found`。

客户端错误返回 `{"ok":false,"code":"...","message":"..."}`。无效请求返回 `400`，不存在的资源返回 `404`，冲突操作返回 `409`，执行名额耗尽返回 `429`。过期的执行标识或不匹配的回复会触发冲突。投递结果不确定时，保留原命令标识用于重试。

## 快照、历史与观察

1. 请求 `/snapshot?workspace=...&agent=...`。JSON 响应包含 `session_id`、`workspace`、已提交 `cursor`、所选 `agent` 快照、`agents` 摘要、`pending` 输入和模型 `selection`。模型设置解析失败时返回 `selection_error`。Agent 状态包含相应的执行标识以及待决问题或授权。每个待接纳条目都含有 `receipt` 和 `input`。Agent 不存在时返回 `404` 和 `agent_not_found`。`/agents?workspace=...` 可直接取得所有 Agent 摘要及游标。加上 `progress=true` 可请求快照游标处已观察到的执行阶段。`progress.phase` 可以是 `waiting_for_model`、`thinking`、`streaming`、`tool_arguments`、`running_tools`、`retrying_model`、`retry_stopped` 或 `retry_decision_unknown`。重试阶段会带有 `retry` 对象，包含持久化的尝试编号、失败类型或恢复原因、请求标识和可用策略元数据。停止重试时 `will_retry` 为 `false`。
2. 请求 `/history?workspace=...&agent=...&before=CURSOR_PLUS_ONE&limit=...`，取得截至该快照、按升序排列的最新一页。用返回的 `next_before` 读取上一页。`before` 为排他游标。`entries` 只包含活动对话历史；单独的 `rewind_groups` 默认是空数组，包含 `rewind_event_id`、原始时间线位置 `target_event_id`、`agent`、可选的显示轮次 `turn_number` 和 `entry_count`。组标题按目标消息的原始位置与活动条目共同分页，并计入页面条目数与字节上限。重试决定和模型恢复会以 `type:"retry"` 条目及结构化 `retry` 对象呈现；历史记录不代表重试仍在进行。
3. 展开某组时按需请求 `/history/rewinds/{rewind_event_id}?workspace=...&agent=...&before=...&limit=...`。必须提供与该组匹配的 `agent`。`before` 是可选的排他游标，`limit` 与 `/history` 一样有界。响应包含 `entries` 和 `next_before`，并复用普通历史条目的显示投影。重叠范围中的每个条目只归属于同一 Agent 的第一个适用回退。只读分页不会把隐藏条目放回活动历史或模型上下文；后续回退也不会使早期回退组无法查看。
4. 连接 `/observe?workspace=...&agent=...&after=CURSOR`，接收包含快照之后提交记录的 NDJSON。省略 `agent` 可以观察整个 Session。从已提交的条目更新续读游标。断开或落后时，从最后的已提交游标重连。对所选 Agent 设置 `include_agent_activity=true`，同一订阅即可收到其他 Agent 的活动。其增量文本及结算预览有长度上限，工具输出仅在切换到对应 Agent 后提供。重连后，Agent 摘要提供最近的输出或错误预览。

所选 Agent 的 `agent.usage` 包含累计 token 数，以及报告输入或输出量的请求数。`cache_measured_input_tokens` 和 `cache_measured_read_tokens` 分别汇总同一批提供有效成对计数的请求中的输入与缓存读取量。有测量时，`latest_request` 提供同一次请求的 `input_tokens`、`output_tokens`、`context_window_tokens` 和 `elapsed_us`。旧记录可能缺少可选测量字段。[终端用量字段](cli.md#终端界面)

快照的 `session_costs` 汇总同一游标下所有保留的 Agent。`reported_nanodollars` 和 `estimated_nanodollars` 的单位是十亿分之一美元。`unknown_requests` 是金额未知的请求尝试数。

有记录时，`agent.turn_started_at_ms` 是最近轮次首次接纳输入的时间，单位为自 Unix 纪元起的毫秒数。轮次结算后，`agent.turn_elapsed_ms` 提供该轮最终耗时，单位为毫秒。新轮次的已提交 `inputs_accepted` 帧也可带有 `turn_started_at_ms`。

`committed` 行包含 `id` 和 `kind`，适用时还包含 Agent 地址、对话条目或状态变更标记。`live` 行包含 `agent`、`execution_id`、`turn`、`cursor` 和临时 `event`。核对它们与当前视图的标识，收到正式结果后替换临时预览。`lagged` 行包含跳过的临时观察数 `skipped`。观察流还可以发送 `heartbeat` 或 `error` 行。观察落后后仍可读取持久状态。

已提交帧可通过 `delivery` 将队列变化关联到命令标识。`accepted` 带有 `input_ids`，`withdrawn` 带有 `input_id`，`replaced` 带有 `input_id` 和 `replacement_id`。

带 `progress=true` 的快照可以包含所选 Agent 的 `reply_preview`，其中有 `execution_id`、`turn`、可选的 `turn_number`、`phase`、`text` 和 `truncated`。阶段为 `thinking`、`streaming` 或 `tool_arguments`。文本是活动请求的可见回复 UTF-8 前缀，最多 64 KiB。`truncated` 表示回复已经超过保留前缀的上限。所选 Agent 的可选字段 `reasoning_preview` 保存服务方公开的思考文字或摘要的有界前缀。Responses 可通过流或最终输出中带类型的思考和摘要文本提供这些内容。不透明思考状态仅用于报告活动，并留在 provider 重放记录中。实时 `reasoning_preview` 事件追加文本，已提交的助手条目也可包含此字段。

重试进度投影支持持久化的 `runtime.model_retry` 和 `model.recovery` 记录。此分支不包含扩展 `runtime.model_retry` 格式的核心生产端更新，因此当前运行时事件仍使用旧格式。旧记录缺少 `phase` 时按 `started` 处理；旧记录缺少 `will_retry` 时沿用 started 事件语义。扩展格式仅以 `will_retry` 表示是否会发送下一请求。`delay_ms` 仅在 `phase:"waiting"` 时表示策略等待时长；客户端可结合该事件的 `ts` 推导剩余时间，不得从供应商响应头推断等待时间。

在同一快照游标处，`agents` 中其他 Agent 的条目也可包含 `progress.phase`。`streaming` 条目的 `progress.preview` 包含该 Agent 当前回复开头的 UTF-8 文本，最多 512 字节。`thinking` 与 `tool_arguments` 仅报告活动。

所选 Agent 的回复预览和对应的 `reply_preview` 事件包含 `output_bytes`，统计已观察到的可见回复字节，包括超出保留前缀的部分。如果有 `output_tokens`，它是上游为当前请求报告的最新累计输出量。实时 `model_output_usage` 事件更新所选 Agent 的同一个计数。每次新请求或重试从自己的计数开始。

指定 Agent 且设置 `include_agent_activity=true` 时，观察流也可以发送 `event.type` 为 `reply_preview` 的 `live` 行。阶段放在 `event.phase`，前缀放在 `event.content`，截断标志放在 `event.truncated`。用该前缀替换对应的内存预览，再应用后续增量。取得前缀与开始接收新增量共用同一接入边界。在此订阅范围内，实时 `thinking` 与 `tool_arguments` 事件仅报告活动。工具从已提交的派发开始执行。续读游标只随已提交记录推进。

历史条目包含持久的 `id`、Agent 地址、可选的 Turn 标识、在该 Agent 内的显示序号（已知时提供 `turn_number`），以及 `user`、`assistant`、`tool_call`、`tool_result`、`question`、`permission`、`question_reply`、`permission_reply`、`status` 或 `error` 等 `type`。工具结果可以包含摘要、结构化详情以及保留的 `output_id`。已提交条目也可以带有运行时 `operation` 或记录该轮结果的 `settlement`。

在 `user` 条目中，`agent` 标识接收者。可信运行时输入记录了发送 Agent 时，`from` 包含该 Agent 在 Session 内的地址。开启一轮的输入也保留此字段。

显式打断可以产生 `status` 为 `"interrupted"` 的 `status` 条目，包含 `execution_id`，也会提供已知的轮次标识和序号。`text` 保留最多 64 KiB 的中断回复，未保留可见正文时为 `null`。`truncated` 表示中断回复超过上限。原始日志可用 `stop_event_id` 将 `turn.cancelled` 对应到停止记录。

已结算的轮次可以有 `turn_summary` 条目。显式打断轮次时，中断的 `status` 条目可以附带同样的摘要。`turn_summary` 对象的 `tool_calls` 是已提交的工具调用数，包括等待授权的调用。可用测量包括从接纳到结算的 `elapsed_ms`、所有模型请求均报告相应计数时的 `input_tokens` 或 `output_tokens`，以及按成对输入与缓存读取计数计算的 `cache_hit_percent`。

Agent 快照和摘要可以包含 `admitted_task` 和 `queued_task`。这两个可为空的字符串分别摘录最近接纳的任务输入和仍在排队的最新任务输入，摘录长度有限。任务摘录不含问题回复。排队数量和待决事项分别读取 `pending_inputs`、`question` 和 `permission`。

回复 `ask_user` 或待决工具授权时，使用快照里的请求标识。并发的回复逐一核对同一待决请求。重开 Session 会保留待决请求和排队输入。已派发但没有确认结果的工具可能已经修改文件或触发副作用。继续中断的工作前检查工作区。[Session](../how-it-works/sessions.md)

## 原始事件与工具输出

`/events?workspace=...&after=0&limit=100` 返回 `events` 和 `next_after`，供核查原始事实。每条记录含 `id` 和 `payload`。可用 `agent` 过滤扫描页，即使没有匹配条目，`next_after` 也会前移。日志结构见 [Session 事件](events.md)。

持久事件页可能包含直接 `Query` 或子 Agent 查询因输出上限重试产生的 `model.recovery`。这些记录遵循同一 `agent` 过滤规则，含义见 [Session 事件](events.md)。

通过 `/outputs/{output_id}` 读取已知工具输出，传入 `workspace` 及可选的 `agent`（默认 `main`）、`offset` 和 `max_bytes`。Agent 必须持有或收到过该输出。可用 `search` 在读取范围内作字面搜索。响应将数据块放在 `output` 中。当 `output.has_more` 为 true 时，传入 `output.next_offset` 续读。[保留输出](tools.md)

## 后台 Job

Job 路由均要求 `workspace` 和 `agent` 查询参数。`GET /sessions/{id}/jobs` 返回 `{ "jobs": [...] }`，其中包含该 Agent 最近的 Job 和所有运行中 Job 的元信息。单个 Job 查询直接返回快照，终态 `result` 可用时一并返回。若快照包含 `output_id`，可通过输出端点读取保留的命令输出。查询未知 Job 或其他 Agent 的 Job 会返回 `404` 和 `job_not_found`。

快照中的 `title` 在状态变化后仍保留原始显示标题。`summary` 描述当前执行状态或终态结果。

`elapsed_ms` 从派发开始计时。查询运行中的任务会返回观察时的用时，查询终态任务会在有测量记录时返回截至结束的用时。`started_at` 和 `finished_at` 是事件游标。

`DELETE /sessions/{id}/jobs/{job_id}` 等待指定执行结束并返回结果快照。对已终结的 Job 重复请求会返回已有结果。两个 GET 路由可以直接读取已关闭的 Session。若本服务尚未打开 Session，取消其中未终结的 Job 会返回 `409` 和 `job_not_active`。

归属 Agent 的历史和观察流包含 `job_finished` 条目，带有 `job_id`、`status`、`summary`、原始 `turn` 以及可用的 `output_id`。此条目可能在对应的 Turn 结束后到达。

## 命名值

`GET /sessions/{id}/values?workspace=...&agent=...` 返回 `{ "values": [...] }`。每项含 `name`、`kind` 和 `reference`（`id`、`offset`、`bytes`）。使用同一 Agent 的输出端点可读取正文。

`PUT /sessions/{id}/values/{name}` 接受包含 `workspace`、`agent`、`source` 和可选 `replace`（默认 `false`）的 JSON 对象。`source` 包含 `kind` 和 `value`。`kind` 可取 `text` 表示 UTF-8 字符串、`json` 表示 JSON 值、`bytes` 表示字节数值数组，或 `output` 表示已有输出 ID。对于新 Session，可以用 `create` 传入初始 `main` Agent 的配置，在首次输入前保存。响应返回保存的值元信息。

对已有 Agent 使用 `send` 时，可在 `values` 中传入所选的 `reference` 对象。命令将这些精确引用绑定到提交的输入。见[命名会话值](tools.md#命名会话值)。

## 查找模型和工作区

`/models` 返回 `default_tier`、`connections` 和 `tiers`。路由包含 `name`、`connection`、`model` 和 `purpose`，以及请求地址、生效设置、能力来源和请求的输出类型，或解析错误。`/models/resolve` 接收候选 Agent 的 `tier`、`connection`、`model`、`reasoning_effort`、`service_tier`、`output` 和 `max_output_tokens`；返回的 `selection` 分别提供模型声明和适配器媒体支持。

`/workspaces` 返回 `workspaces`。向 `/workspaces/resolve` 将已有目录的绝对路径作为 `workspace` 传入，可取得规范路径。`/workspaces/files` 和 `/workspaces/skills` 还接受 `query` 过滤参数，分别返回 `paths` 和 `skills`。匹配的文件是工作区相对路径，匹配的 Skill 包含名称和描述。
