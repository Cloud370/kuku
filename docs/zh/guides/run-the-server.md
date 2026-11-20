# 运行服务

## 受管本地服务

在项目目录启动 `kuku`，或用 `kuku run` 提交脚本任务。客户端会查找或启动所选 kuku 数据目录的回环服务。来自不同项目的客户端共用该服务，并各自传入解析后的工作区。关闭终端客户端后，服务仍会继续执行。

运行 `kuku server status` 查看当前构建、协议和生效配置。安装新二进制或修改环境变量后，用 `kuku server restart` 让服务加载新的构建或环境。如果服务报告还有执行或清理工作，等它们结束后再试。重启时可以传入 `--config PATH` 切换配置文件。[升级 kuku](upgrade-kuku.md)

## 在浏览器接着工作

使用发行包或内嵌 Web 资源的源码构建时，在工作区运行 `kuku web`。在浏览器打开已有 Session，或者启动时传入 `--session SESSION_ID --agent ADDRESS`。在终端也可以针对当前 Agent 使用 `/web`。浏览器和终端可以观察相同的工作，回复相同的待决请求。

受管本地服务通过一次性票据认证。如果显式选择了内嵌 Web 资源的服务，在浏览器打开登录页后输入服务访问令牌。[CLI 参考](../reference/cli.md#kuku-server-和-kuku-web) · [HTTP 认证](../reference/server-api.md#认证与版本)

在浏览器中按 Enter 发送或插话，按 Shift+Enter 换行，按 Alt+Enter 排到下一轮。用 Agent 选择器设置接收者。点击 Stop 打断所选执行，Continue 继续，Withdraw 撤回排队输入。回执丢失后，可以在页面重新查询，或重发原命令。[Session](../how-it-works/sessions.md)

## 显式启动服务

在服务端机器初始化 kuku 数据目录并设置模型凭据。选择服务访问令牌，在终端环境中设置 `KUKU_SERVER_PASSWORD`，再运行以下命令。服务应在客户端连接期间保持运行。

```bash
kuku server --listen 127.0.0.1:17777
```

在另一个终端中将 `KUKU_TOKEN` 设为相同的令牌。把工作区替换为服务端机器上的绝对路径。

```bash
kuku --remote http://127.0.0.1:17777 --workspace /code/my-project run "Summarize this directory"
```

显式指定的地址在连接失败时仍然有效。连接另一台机器时，使用经过认证的 HTTPS 入口或安全隧道。服务端配置决定模型访问和工具权限。[连接与工作区选项](../reference/cli.md#连接与工作区选项)

## 检查健康状态与调用 API

```bash
curl http://127.0.0.1:17777/health
```

此接口返回构建和协议版本。调用 Session 命令和观察接口时，发送 `Authorization: Bearer TOKEN`。回执丢失后使用同一 Session 和命令标识恢复，观察断开后可通过已提交游标续读。[HTTP 服务接口](../reference/server-api.md)
