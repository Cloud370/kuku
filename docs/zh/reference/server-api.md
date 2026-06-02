# Server API

## 启动 Server

默认监听地址：

```text
127.0.0.1:17777
```

非 loopback 监听器需要 `--password`。

## 认证

- loopback 客户端会绕过密码检查。
- 远程客户端必须发送 `Authorization: Bearer <token>`。
- 认证失败会返回 HTTP 200，内容为：

```json
{"ok": false, "code": "auth_required", "message": "password required"}
```

## 端点

| Method | Path | Purpose |
|---|---|---|
| `GET` | `/health` | 健康检查 |
| `POST` | `/runs` | 启动一次 run 并流式返回 NDJSON |
| `DELETE` | `/runs/{id}` | 取消一次 run |
| `POST` | `/runs/{id}/responses` | 回复一次交互请求 |
| `GET` | `/sessions` | 列出 Session |
| `DELETE` | `/sessions/{id}` | 删除一个 Session |
| `GET` | `/sessions/{id}/events` | 读取已持久化事件和可选的活动流 |

请求体大小限制为 10 MB。

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
  "tier": "optional-tier-name"
}
```

规则：

- `workspace` 为必填，且必须存在
- `session_id` 为可选
- `tier` 为可选

成功响应：内容类型为 `application/x-ndjson` 的 NDJSON 流。

## NDJSON 线路事件

server 当前发出的顶层事件类型：

- `run_start`
- `turn_start`
- `model_request`
- `text`
- `thinking`
- `tool_start`
- `tool_output`
- `tool_end`
- `permission`
- `done`
- `cancelled`
- `error`

示例：

```json
{"type":"run_start","run_id":"..."}
{"type":"text","content":"hello"}
{"type":"done","session_id":"...","text":"done","turn":1,"usage":null}
```

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

有效的 `choice` 值：

- `once`
- `session`
- `project`
- `deny`

## `GET /sessions`

查询参数：

- 可选的 `workspace`

成功响应：

```json
{
  "ok": true,
  "sessions": []
}
```

每个 Session 项包含 `session_id`、`workspace`、`title`、`created_at`、`turn_count`、`status`、`mtime` 和 `size`。

## `DELETE /sessions/{id}`

查询参数：

- 可选的 `workspace`

常见错误码：

- `session_locked`
- `session_not_found`
- `internal`

## `GET /sessions/{id}/events`

查询参数：

- 可选的 `after` 整数事件 id
- 可选的 `workspace`

响应形状：

- 仅历史事件：JSON 数组
- 历史事件加实时缓冲行：带 `events` 和 `active_stream` 的对象
