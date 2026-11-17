# 运行服务

需要关闭客户端后任务仍能继续时，通过 HTTP server 发起任务，并让 server 进程保持运行。当前前台 CLI 会单独打开 SDK Session。

## 启动服务

先初始化 kuku 并设置所选连接的 API 密钥。在项目目录启动 server，保持这个终端运行。[快速开始](../start/quickstart.md)

```bash
kuku server --listen 127.0.0.1:17777
```

可选参数：

- `--config <path>`
- `--password <token>`
- `--max-concurrent-runs <n>`

server 默认读取 `~/.kuku/config.toml`。如果 `kuku init` 将配置写在自定义的 `KUKU_HOME` 下，请通过 `--config` 传入该文件。Session 存储仍使用 `KUKU_HOME`，HTTP 请求中的工作目录指的是服务端机器的路径。[CLI 参考](../reference/cli.md)

## 远程访问规则

要监听非回环地址，先把终端变量 `server_password` 设为你选择的访问口令。远程客户端在 `Authorization: Bearer` 请求头中传入同一口令。

```bash
kuku server --listen 0.0.0.0:17777 --password "${server_password:?Set server_password first}"
```

## 健康检查

```bash
curl http://127.0.0.1:17777/health
```

## 通过 HTTP 执行任务

另开一个终端，向本地 server 发送任务。这里的 `.` 表示启动 server 时所在的目录。

```bash
curl -N -H 'Content-Type: application/json' \
  -d '{"prompt":"Summarize this directory","workspace":"."}' \
  http://127.0.0.1:17777/runs
```

返回的第一行 NDJSON 包含 Session ID 和输入回执。可添加 `connection` 与 `model`，或通过 `tier` 选择模型。`agent` 字段选择稳定地址，默认为 `main`。其他地址各有独立的上下文。

服务使用公开 Session SDK，默认工具策略为 trusted-local。HTTP 订阅者只观察进度，断开客户端后执行仍继续。使用 `DELETE /runs/{id}` 取消并等待清理。

精确请求和响应格式见 [Server API](../reference/server-api.md)。

## 继续或检查 Session

使用 `after`、`limit` 读取持久事件页，并将 `next_after` 用作下一游标。延后问题/授权必须通过回复端点携带精确请求标识。等待不占用模型执行槽位。重启服务后，通过 `POST /sessions/{id}/responses` 携带原工作区、Agent 和请求标识回复。

服务还提供：

- `GET /sessions`
- `GET /sessions/{id}/events`
- `GET /sessions/{id}/conversations`
- `DELETE /sessions/{id}`
- `POST /runs/{id}/responses`
- `POST /sessions/{id}/responses`

## 相关页面

- [Server API](../reference/server-api.md)
- [事件](../reference/events.md)
