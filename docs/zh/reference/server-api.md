# Server API

## Start the Server

默认监听地址：

```text
127.0.0.1:17777
```

非 loopback 监听要求 `--password`。

## Authentication

- loopback 客户端跳过密码检查。
- 远程客户端必须发送 `Authorization: Bearer <token>`。
- 认证失败时返回 HTTP 200：

```json
{"ok": false, "code": "auth_required", "message": "password required"}
```

## Endpoints

| Method | Path | Purpose |
|---|---|---|
| `GET` | `/health` | 健康检查 |
| `POST` | `/runs` | 启动运行并流式返回 NDJSON |
| `DELETE` | `/runs/{id}` | 取消运行 |
| `POST` | `/runs/{id}/responses` | 回复交互请求 |
| `GET` | `/sessions` | 列出 Sessions |
| `DELETE` | `/sessions/{id}` | 删除一个 Session |
| `GET` | `/sessions/{id}/events` | 检查持久账本和可选实时流 |
| `GET` | `/sessions/{id}/conversations` | 列出一个 Session 内的 conversation 线程 |

请求体限制为 10 MB。

## `GET /health`

响应：

```json
{
  "ok": true,
  "version": "0.1.0",
  "workspace": "/current/server/working/dir"
}
```

## `POST /runs`

请求体：

```json
{
  "prompt": "check this project",
  "workspace": "/code/my-project",
  "session_id": "optional-existing-session",
  "conversation": "optional-address",
  "tier": "optional-tier-name"
}
```

规则：

- `workspace` 必填，且必须存在
- `session_id` 可选
- `conversation` 可选，默认是 `main`
- `tier` 可选

复用相同的 `session_id` 和 `conversation` 就表示继续同一线程。

成功响应是内容类型为 `application/x-ndjson` 的 NDJSON 流。

## NDJSON Wire Events

顶层流事件类型：

- `run_start`
- `turn_start`
- `model_request`
- `text`
- `thinking`
- `tool_start`
- `tool_output`
- `tool_end`
- `permission`
- `log`
- `done`
- `cancelled`
- `error`

示例：

```json
{"type":"run_start","run_id":"..."}
{"type":"text","content":"hello"}
{"type":"done","session_id":"...","text":"done","turn":1,"usage":null,"model_request_count":1,"thinking_duration_ms":0,"tool_summary":{"total_calls":0,"names":[],"denied":0,"errors":0,"rounds":0}}
```

`tool_start` 包含 `kind` 元数据。

- 普通工具：`"kind":"simple"`
- 命令工具：`"kind":{"command":{"pid":42}}`
- agent 工具：`"kind":{"agent":{"conversation":"review","binding_id":"sha256:..."}}`

`done` 会包含运行指标：

| Field | Type | Description |
|---|---|---|
| `model_request_count` | `u64` | 本 Session 中的模型 API 调用次数 |
| `thinking_duration_ms` | `u64` | thinking 块累计耗时 |
| `tool_summary.total_calls` | `u64` | Tool 调用总数，包括 blocked |
| `tool_summary.names` | `string[]` | 按首次出现顺序记录的唯一 Tool 名称 |
| `tool_summary.denied` | `u64` | 权限拒绝次数 |
| `tool_summary.errors` | `u64` | 返回错误状态的 Tool 执行次数 |
| `tool_summary.rounds` | `u64` | model -> tools -> result 的循环次数 |

运行会以 `done`、`cancelled` 或 `error` 结束。

`log` 记录是 active stream 中 host 可见的可观测性数据，不会作为持久 Session 事实写入。

## `DELETE /runs/{id}`

响应：

```json
{"ok": true}
```

或者：

```json
{"ok": false, "code": "session_not_found", "message": "run not found"}
```

## `POST /runs/{id}/responses`

请求体：

```json
{
  "interaction_id": "req_1",
  "choice": "once"
}
```

有效 `choice` 值：

- `once`
- `session`
- `project`
- `deny`

## `GET /sessions`

查询参数：

- 可选 `workspace`

成功响应：

```json
{
  "ok": true,
  "sessions": []
}
```

每个 session 项包含 `session_id`、`workspace`、`title`、`created_at`、`turn_count`、`status`、`mtime` 和 `size`。

## `GET /sessions/{id}/conversations`

查询参数：

- 可选 `workspace`

成功响应：

```json
{
  "ok": true,
  "session_id": "sess_123",
  "conversations": [
    {
      "conversation": "main",
      "binding_id": null,
      "status": "completed:3"
    }
  ]
}
```

每个 conversation 项都会报告：

- `conversation`
- `binding_id`
- `status`，取值为 `opened`、`active:<turn>`、`completed:<turn>`、`cancelled:<turn>` 或 `interrupted:<turn>`

## `DELETE /sessions/{id}`

查询参数：

- 可选 `workspace`

常见错误码：

- `session_locked`
- `session_not_found`
- `internal`

## `GET /sessions/{id}/events`

查询参数：

- 可选 `after` 整数事件 id
- 可选 `conversation` address
- 可选 `workspace`

响应形态：

- 只有历史事件：JSON 数组
- 历史事件加实时缓存流：包含 `events` 和 `active_stream` 的对象

持久 `/events` 数据就是来自 `events.jsonl` 的 Session 账本。

- 省略 `conversation` 可检查完整账本
- 传 `conversation=review` 可过滤单个线程
- 传 `after=<id>` 可做增量读取

当存在活动流时，`active_stream` 还可能包含 host 可见的运行时记录，包括 `log` 记录。持久 Session 的语义仍然是以事实为中心。
