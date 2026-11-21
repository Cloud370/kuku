# 升级 kuku

## 更新可执行文件

通过 Cargo 安装测试版时，重新安装仓库当前版本提供的统一二进制。

```bash
cargo install --locked --git https://github.com/Cloud370/kuku --force kuku-app
```

使用带标签的发行版时，选择目标版本的清单，并使用对应平台的安装脚本。[安装已发布版本](../start/install.md#安装已发布版本)

## 重新连接受管服务

照常启动新程序。同一数据目录复用同构建的受管服务，旧发行版本的服务空闲时会自动替换。启动同一发行版本的另一构建时，会选用本次启动的构建。较旧发行版本的客户端会提示后台正在运行的较新版本。要并行使用多个版本，请使用各自的数据目录。

后台还有任务时，TUI 会显示等待提示，等任务结束后自动连接。版本切换让会话断开时，原窗口会提示正在运行的版本。重连时仍使用窗口原有的版本。用 `kuku server status` 查看运行中的构建和任务。正常替换会保留已提交的会话历史、待回应的请求和排队输入。[运行服务](run-the-server.md) · [会话](../how-it-works/sessions.md)

## 核对配置

```bash
kuku --help
kuku config validate
```

kuku 将配置保存在 `config.toml` 中。使用 `kuku config show` 查看当前模型档和连接。[配置指南](../start/configuration.md)
