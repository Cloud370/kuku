# Run the Server

当其他客户端需要通过 HTTP 访问 kuku 时，就使用 server。

## Start the Server

```bash
kuku server --listen 127.0.0.1:17777
```

可选参数：

- `--config <path>`
- `--password <token>`
- `--max-concurrent-runs <n>`

准确参数见 [CLI](../reference/cli.md)。

## Remote Access Rule

如果你绑定到非 loopback 地址，就必须提供 `--password`。

示例：

```bash
kuku server --listen 0.0.0.0:17777 --password <token>
```

## Check Health

```bash
curl http://127.0.0.1:17777/health
```

## Run a Task Over HTTP

发送 `POST /runs`，并附带 `prompt` 和 `workspace`。响应为 NDJSON。

准确的请求与响应格式见 [Server API](../reference/server-api.md)。

## Continue or Inspect Sessions

server 还会暴露：

- `GET /sessions`
- `GET /sessions/{id}/events`
- `DELETE /sessions/{id}`
- `POST /runs/{id}/responses`

## Related Pages

- [Server API](../reference/server-api.md)
- [Events](../reference/events.md)
