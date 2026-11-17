# 运行服务

其他客户端需要通过 HTTP 访问 kuku 时使用服务宿主。

## 启动服务

```bash
kuku server --listen 127.0.0.1:17777
```

可选参数：

- `--config <path>`
- `--password <token>`
- `--max-concurrent-runs <n>`

精确参数见 [CLI](../reference/cli.md)。

## 远程访问规则

绑定非回环地址时必须设置 `--password`。

```bash
kuku server --listen 0.0.0.0:17777 --password <token>
```

## 健康检查

```bash
curl http://127.0.0.1:17777/health
```

## 通过 HTTP 执行任务

向 `POST /runs` 提交 `prompt` 和 `workspace`。可添加 `connection` 与 `model`，或 `tier` 精确选择模型。`agent` 选择稳定地址，默认 `main`；其他地址具有独立上下文。响应为 NDJSON，首项包含 Session 标识和持久回执。

服务使用公开 Session SDK，默认工具策略为 trusted-local。HTTP 订阅者只观察进度，断开客户端后执行仍继续。使用 `DELETE /runs/{id}` 取消并等待清理。

精确请求和响应格式见 [Server API](../reference/server-api.md)。

## 继续或检查 Session

使用 `after`、`limit` 读取持久事件页，并将 `next_after` 用作下一游标。延后问题/授权必须通过回复端点携带精确请求标识。等待不占用模型执行槽位。

服务还提供：

- `GET /sessions`
- `GET /sessions/{id}/events`
- `GET /sessions/{id}/conversations`
- `DELETE /sessions/{id}`
- `POST /runs/{id}/responses`

## 相关页面

- [Server API](../reference/server-api.md)
- [事件](../reference/events.md)
