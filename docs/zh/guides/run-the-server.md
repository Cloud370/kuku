# 运行服务

## 受管本地服务

运行 `kuku` 或 `kuku run` 时，客户端会自动查找或拉起对应 `KUKU_HOME` 数据目录的本机回环服务。不同项目目录的客户端共用这一个服务，各自传入自己的工作区路径；关掉终端后，服务仍会继续跑完已接下的工作。

- `kuku server status`：查看运行中的构建版本、协议版本和生效配置路径
- `kuku server restart`：加载新二进制或更新后的环境变量（如果服务正忙，等任务结束后重试）
- `kuku server restart --config PATH`：切换受管服务使用的配置文件

版本升级时的接管规则见[升级 kuku](upgrade-kuku.md)。

## 在浏览器里使用 Web UI

如果使用的是发行包，或内嵌了前端资源的源码构建，在工作区运行 `kuku web`（可带 `--session SESSION_ID --agent ADDRESS`），或者直接在 TUI 里对当前 Agent 输入 `/web`。浏览器和终端可以同时观察同一个 Session，并回复同一个待决请求。

受管本地启动会用一次性票据自动登录；显式指向外部服务时，浏览器会打开登录页让你输入该服务的访问令牌。[CLI 参考](../reference/cli.md#kuku-server-与-kuku-web) · [HTTP 认证](../reference/server-api.md#认证与版本)

浏览器快捷键与 TUI 一致：**Enter** 发送或插话，**Shift+Enter** 换行，**Alt+Enter** 排到下一轮。界面上可以切换接收 Agent，点 **Stop** 中断、**Continue** 继续、**Withdraw** 撤回排队输入；若请求回执丢失，页面支持重新查询或重发原命令。[Session 模型](../how-it-works/sessions.md)

## 显式启动服务

在服务端机器上初始化 `KUKU_HOME`、配好模型凭据，设置 `KUKU_SERVER_PASSWORD` 作为访问令牌后保持服务运行：

```bash
kuku server --listen 127.0.0.1:17777
```

在客户端设置相同的 `KUKU_TOKEN` 连接，注意 `--workspace` 要传服务端机器上的绝对路径：

```bash
kuku --remote http://127.0.0.1:17777 --workspace /code/my-project run "Summarize this directory"
```

显式传了 `--remote` 后，就算连不上也不会静默回退到本地服务。跨机器连接请走带认证的 HTTPS 反向代理或安全隧道。模型访问与工具权限一律由服务端配置决定。[连接与工作区选项](../reference/cli.md#连接与工作区选项)

## 健康检查与 HTTP API

```bash
curl http://127.0.0.1:17777/health
```

`/health` 无需鉴权，返回构建和协议版本。Session 命令和事件流接口需要携带 `Authorization: Bearer TOKEN`。回执丢失时用原 Session ID 和命令 ID 重试即可保证幂等，事件流断开后传上次已提交游标续读。[HTTP 服务接口](../reference/server-api.md)
