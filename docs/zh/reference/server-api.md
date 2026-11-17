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
| `GET` | `/sessions/{id}/observe` | 观察游标之后的已提交条目和临时进度 |
| `GET` | `/sessions/{id}/events` | 分页读取原始已提交事实 |
| `GET` | `/sessions/{id}/outputs/{output_id}` | 有界读取保留的工具输出 |
| `GET` | `/models` | 已配置的模型目录 |
| `GET` | `/models/resolve` | 解析候选 Agent 配置的生效设置 |
| `GET` | `/workspaces` | 列出工作区 |
| `GET` | `/workspaces/resolve`、`/workspaces/files`、`/workspaces/skills` | 解析服务端路径，或查找工作区文件与 Skill |

状态响应包含构建和协议版本、`instance_id`、`pid`、生效的 `data_dir` 和 `config_path`、`draining` 以及 `busy`。受管服务重启返回 `202` 表示已释放 writer，进程正在停止。`409` 和 `service_busy` 在 `busy` 列出正在进行的任务。受管客户端会等旧进程释放服务租约后启动替代进程。

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
| `send` | `text`、可选 `timing`、可选 `create` | 持久投递。默认的 `steer` 可在下一个安全的模型请求边界接纳，`next_turn` 等待下一轮。 |
| `create_agent` | `config` | 按地址创建拥有独立上下文的 Agent。 |
| `configure` | `config` | 在安全边界修改模型或默认值。 |
| `reply_question` | `request_id`、`text` | 回答精确的待决问题。 |
| `reply_permission` | `request_id`、`allow` | 决定精确的待决授权。 |
| `withdraw` | `input_command_id` | 撤回排队输入或返回其当前状态。 |
| `advance` | | 显式继续 Agent 的工作。 |
| `cancel` | `execution_id` | 取消观察到的那次执行并等待清理。 |

命令成功时返回含 `ok`、`receipt` 和 `replayed` 的 JSON。回执包含 `command_id`、`event_id` 和 `agent`。输入命令可以报告 `status` 为 `queued`、`accepted` 或 `withdrawn`。持久回执表示已经投递。模型接纳和执行结算分别记录。

在给已有 Agent 发送输入的同时修改模型选择时，传入 `config_patch`。若脚本需要在已有 Session 中创建缺失的 Agent，还要传入 `create_agent_if_missing: true`。新 Session 使用 `create` 设置初始 Agent，也可以用 `prompts_dir` 指定服务端自定义 Prompt 资源的绝对路径。

通过 `send` 的 `bootstrap_skill` 为该条输入指定 Skill，在接纳时应用。

如果没有收到响应，带上 `workspace` 查询回执接口。缺失的回执返回 HTTP `404` 和 `receipt_not_found`。要重发时，保持原 Session、Agent、命令标识和请求内容。重放已提交命令会返回原回执。以相同的命令标识提交冲突内容会返回 HTTP `409`。`?replay_only=true` 用于校验重放请求，命令尚未提交时返回 `receipt_not_found`。

客户端错误返回 `{"ok":false,"code":"...","message":"..."}`。无效请求返回 `400`，不存在的资源返回 `404`，冲突操作返回 `409`，执行名额耗尽返回 `429`。过期的执行标识或不匹配的回复会触发冲突。投递结果不确定时，保留原命令标识用于重试。

## 快照、历史与观察

1. 请求 `/snapshot?workspace=...&agent=...`。JSON 响应包含 `session_id`、`workspace`、已提交 `cursor`、所选 `agent` 快照、`agents` 摘要、`pending` 输入和模型 `selection`。模型设置解析失败时返回 `selection_error`。Agent 状态包含相应的执行标识以及待决问题或授权。每个待接纳条目都含有 `receipt` 和 `input`。Agent 不存在时返回 `404` 和 `agent_not_found`。`/agents?workspace=...` 可直接取得所有 Agent 摘要及游标。
2. 请求 `/history?workspace=...&agent=...&before=CURSOR_PLUS_ONE&limit=...`，取得截至该快照、按升序排列的最新一页。用返回的 `next_before` 读取上一页。`before` 为排他游标。
3. 连接 `/observe?workspace=...&agent=...&after=CURSOR`，接收快照之后的 NDJSON。省略 `agent` 可以观察整个 Session。从已提交的条目更新续读游标。断开或落后时，从最后的已提交游标重连。

`committed` 行包含 `id` 和 `kind`，适用时还包含 Agent 地址、对话条目或状态变更标记。`live` 行包含 `agent`、`execution_id`、`turn`、`cursor` 和临时 `event`。核对它们与当前视图的标识，收到正式结果后替换临时预览。`lagged` 行包含跳过的临时观察数 `skipped`。观察流还可以发送 `heartbeat` 或 `error` 行。观察落后后仍可读取持久状态。

历史条目包含持久的 `id`、Agent 地址、可选的 Turn，以及 `user`、`assistant`、`tool_call`、`tool_result`、`question`、`permission`、`question_reply`、`permission_reply`、`status` 或 `error` 等 `type`。工具结果可以包含摘要、结构化详情以及保留的 `output_id`。已提交条目也可以带有运行时 `operation` 或记录该轮结果的 `settlement`。

回复 `ask_user` 或待决工具授权时，使用快照里的请求标识。并发的回复逐一核对同一待决请求。重开 Session 会保留待决请求和排队输入。已派发但没有确认结果的工具可能已经修改文件或触发副作用。继续中断的工作前检查工作区。[Session](../how-it-works/sessions.md)

## 原始事件与工具输出

`/events?workspace=...&after=0&limit=100` 返回 `events` 和 `next_after`，供核查原始事实。每条记录含 `id` 和 `payload`。可用 `agent` 过滤扫描页，即使没有匹配条目，`next_after` 也会前移。日志结构见 [Session 事件](events.md)。

通过 `/outputs/{output_id}` 读取已知工具输出，传入工作区及可选 `offset`、`max_bytes`。可用 `search` 在读取范围内作字面搜索。响应将数据块放在 `output` 中。当 `output.has_more` 为 true 时，传入 `output.next_offset` 续读。[保留输出](tools.md)

## 查找模型和工作区

`/models` 返回 `default_tier`、`connections` 和 `tiers`。连接有 `name` 和线路 `protocol`。模型档包含 `name`、`connection`、`model` 和 `purpose`，以及设置与能力或解析错误。向 `/models/resolve` 传入 `tier`、`connection`、`model`、`reasoning_effort` 或 `service_tier` 等模型选择字段，可得到解析后的 `selection`。

`/workspaces` 返回 `workspaces`。向 `/workspaces/resolve` 将已有目录的绝对路径作为 `workspace` 传入，可取得规范路径。`/workspaces/files` 和 `/workspaces/skills` 还接受 `query` 过滤参数，分别返回 `paths` 和 `skills`。匹配的文件是工作区相对路径，匹配的 Skill 包含名称和描述。
