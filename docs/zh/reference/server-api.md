# Server API

## 启动服务

默认地址为 `127.0.0.1:17777`。监听非回环地址需要 `--password`。

## 认证

回环客户端跳过密码检查。远程客户端发送 `Authorization: Bearer <token>`。认证失败以 HTTP 200 返回 `{"ok":false,"code":"auth_required","message":"password required"}`。

默认单用户服务使用 trusted-local 工具授权。嵌入宿主可通过 `AppState.authorizer` 允许、拒绝或延后精确工具动作。

## 端点

| 方法 | 路径 | 用途 |
|---|---|---|
| `GET` | `/health` | 健康检查 |
| `POST` | `/runs` | 提交工作并观察 NDJSON |
| `DELETE` | `/runs/{id}` | 取消并等待所拥有工作的清理 |
| `POST` | `/runs/{id}/responses` | 回复待决问题或授权 |
| `POST` | `/sessions/{id}/responses` | 重开持久 Session 并回复精确待决请求 |
| `GET` | `/sessions` | 列出保留的 Session |
| `DELETE` | `/sessions/{id}` | 删除已关闭的 Session |
| `GET` | `/sessions/{id}/events` | 读取有界持久页及可选近期进度 |
| `GET` | `/sessions/{id}/conversations` | 查看一个 Session 中的稳定 Agent |

请求体上限为 10 MB，SDK 的输入和执行限制仍然生效。

## `GET /health`

```json
{"ok":true,"version":"0.1.0","workspace":"/server/working/directory"}
```

## `POST /runs`

```json
{
  "prompt": "check this project",
  "workspace": "/code/my-project",
  "session_id": "optional-session",
  "agent": "reviewer",
  "connection": "named-connection",
  "model": "exact-model-id",
  "command_id": "optional-caller-command"
}
```

`workspace` 必须是目录。`session_id` 可选，`agent` 默认 `main`。稳定地址为扁平的小写 ASCII 字母、数字、`-` 或 `_`，最多 64 字节。输入仍接受 `conversation` 作为 `agent` 别名。

模型选择复用 SDK 解析器。可选字段为 `tier`、`connection`、`model`、`reasoning_effort`、`service_tier` 和 `max_output_tokens`。`no_agents`、`no_skills` 禁用对应工具面。精确连接按配置名称选择端点，不因协议相同而选用其他连接。重开时不传模型参数会保留该 Agent 的配置；显式修改只影响该 Agent。切换路由时先清除旧的 Agent 局部 reasoning/service 覆盖值，再应用新显式值。

`command_id` 接受 1–100 字节，是 SDK 输入幂等标识，返回的 `run_id` 标识本次 HTTP 执行控制句柄，与 Session 内命令标识分开。回执确认输入已经持久化，执行终结事实随后产生。重复使用同一 Session 和 Agent 延续其上下文；同一 Session 中其他地址具有独立上下文，可并发执行。对已在运行或等待的地址提交新命令会明确失败。对已停止/中断的目标提交新命令会显式推进它。重试同一输入命令时返回 HTTP JSON，包含 `ok`、`replayed: true`、原始持久 `receipt`，以及仍被保留时的 `run_id`。重试不会再占执行槽位、修改配置或重复派发；输入或显式配置的冲突复用会失败。SDK 调用方仍可在安全边界接纳新输入，HTTP 宿主维持每个 Agent 一个活动控制句柄。

新执行响应为 `application/x-ndjson`；仅确认回执时返回 JSON。最后一个 HTTP run 完成后，服务会等待该 Session 中已在运行的 Agent 结算，再关闭资源。等待和中断的 Agent 保持休眠。HTTP 观察者断开后 Session 继续执行，关闭或放慢客户端不会成为执行链的控制者。

## NDJSON 事件

`run_start` 包含 `run_id`、`session_id`、`agent` 和持久输入 `receipt`。瞬时更新包括 `turn_start`、`model_request`、`text`、`thinking`、`tool_start`、`tool_output`、`tool_end` 和 `log`。

`committed` 通过 `id` 和 `kind` 标识持久事件，完整 payload 从事件端点读取。`lagged` 报告丢失的瞬时更新，可使用持久游标重新读取。队列有界：广播最多 256 条，近期缓冲最多 200 条；近期缓冲每个 run 不超过 1 MiB。瞬时进度可能丢失，已提交事实与终结响应仍可通过 Session 日志检查。

问题发送带精确 `id`、`agent` 和 `text` 的 `question` 事件；延后授权发送带匹配标识的 `permission`。每个待决请求标识在一个保留的 HTTP 控制中只通知一次，问题与授权并存时也分别通知；回答问题不会决定授权。等待会释放服务执行槽位，不会因超时隐式授权或拒绝。`question` 和 `permission` 都不是终结事件，原流与 HTTP 控制仍保持打开，等待匹配的回复或取消。

终结事件为 `done`、`cancelled` 或 `error`。`done` 位于持久结算之后，含 `session_id`、`agent`、`text`、`turn`、累计 Provider `usage` 和 Agent `config`。`conversation` 为兼容现有消费者而镜像 Agent 地址。Turn 完成不代表任务成功，应检查真实文件与判定命令。

```json
{"type":"run_start","run_id":"http-run-1","session_id":"...","agent":"main","receipt":{"command_id":"command-1","event_id":3,"agent":"main"}}
{"type":"text","content":"hello","agent":"main"}
{"type":"done","session_id":"...","agent":"main","conversation":"main","text":"done","turn":1,"usage":{"model_requests":1},"config":{}}
```

Provider 用量及未知报告遵循 [Session SDK](../how-it-works/sessions.md)。活动 `log` 为可观察性记录，与持久执行事实分开存放。

## `DELETE /runs/{id}`

取消及清理完成后返回 `{"ok":true}`。run 不存在返回 `session_not_found`；清理失败返回 `cleanup_failed` 和错误消息。服务退出时关闭 Session、保留待决等待、等待清理并报告失败。

## `POST /runs/{id}/responses`

```json
{"interaction_id":"exact-pending-request","agent":"reviewer","choice":"once","command_id":"reply-1"}
```

授权的 `choice` 为 `once` 或 `deny`；问题使用 `answer` 并提供 `text`。`agent` 可选，提供时必须匹配原 run；`conversation` 仍为别名。回复不会隐含 Session/项目范围授权。调用方指定 `command_id` 可安全重试同一回复。错误或过期请求不会推进 Agent。

当前打开的服务 run 通过原流继续。成功时返回 JSON `{"ok":true,"receipt":{...}}`。

## `POST /sessions/{id}/responses`

服务重启后使用持久 Session 标识，无需旧 HTTP `run_id`。请求沿用回复字段，并必须提供 `workspace`：

```json
{"workspace":"/code/my-project","agent":"reviewer","interaction_id":"exact-pending-request","choice":"answer","text":"the requested information","command_id":"reply-1"}
```

Session 必须已存在，`agent` 默认 `main`。宿主重开该 Session，调用匹配的 SDK 回复操作，并在续接期间保留同一运行时。再次等待时，流与控制保持打开，同时释放执行容量。新的 HTTP 执行返回普通 NDJSON 流和新 `run_id`，`run_start.receipt` 指向持久回复。若该 Agent 已有保留的 HTTP 控制，则返回含该 `run_id` 的 JSON 回执，并通过原流继续。重复回复在没有 HTTP 控制时返回 JSON `{"ok":true,"replayed":true,"run_id":null,"receipt":{...}}`，仍有控制时返回其现有 `run_id`。通过保留控制提交新回复时，回执结构相同，`replayed:false`。两种重复路径都不产生第二次续接。错误/过期请求和冲突重试均不会派发模型或工具。

## `GET /sessions`

可选 `workspace` 查询用于过滤。响应为 `{"ok":true,"sessions":[]}`。Session 条目包含 `session_id`、`workspace`、`title`、`created_at`、`turn_count`、`status`、`mtime` 和 `size`。

## `GET /sessions/{id}/conversations`

可选 `workspace` 指定 Session 位置。保留的端点返回 `{"ok":true,"session_id":"...","conversations":[]}`。每项报告 `agent`/`conversation`、`status`、`pending_inputs`、待决 `permission`/`question`、`config` 和 `usage`。读取不会推进执行或获取第二个 Session 运行时。状态遵循[离线账本快照](sdk.md#操作)，仅凭它无法判定活动 HTTP 执行是否已经停止。

## `DELETE /sessions/{id}`

可选 `workspace` 指定位置。常见错误为 `session_locked`、`session_not_found` 和 `internal`。

## `GET /sessions/{id}/events`

查询字段：

- `after`：排他的持久事件游标，默认 0。
- `limit`：最多扫描的记录数，默认 100，接受 1–256。
- `agent` 或 `conversation`：有界账本页内的地址过滤。
- `workspace`：Session 位置。

响应始终包含 `events`、`next_after` 和 `active_stream`。即使过滤排除了所有事件，`next_after` 仍为最后扫描到的账本游标，应将其用于下一页。持久事件保留 `id` 和 `payload`；近期活动进度可为空，不替代持久记录。

持久事件页可能包含直接 `Query` 或子 Agent 查询因输出上限而重试时产生的 `model.recovery`。该事件遵循 `agent`／`conversation` 过滤；含义见 [Session 事件](events.md)。

其他进程持有 Session writer 时可能阻止读取；服务会明确返回资源冲突，不启动第二个 writer。
