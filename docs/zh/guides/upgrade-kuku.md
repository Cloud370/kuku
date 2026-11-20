# 升级 kuku

## 更新可执行文件

通过 Cargo 安装测试版时，重新安装仓库当前版本提供的统一二进制。

```bash
cargo install --locked --git https://github.com/Cloud370/kuku --force kuku-app
```

使用带标签的发行版时，选择目标版本的清单，并使用对应平台的安装脚本。[安装已发布版本](../start/install.md#安装已发布版本)

## 重新连接受管服务

替换已安装的客户端后，正在运行的服务继续完成手头的工作。客户端和服务的协议版本匹配时，可以用新客户端打开 Session。运行 `kuku server status` 可查看服务构建和当前工作。

如果客户端报告协议不匹配，等执行和工具清理结束后，用新二进制运行 `kuku server restart`。繁忙的服务会报告仍在进行的工作，完成后可以重试。正常重启会释放 Session writer，并保留已提交历史、待决请求和排队输入。[运行服务](run-the-server.md) · [Session](../how-it-works/sessions.md)

## 核对配置

```bash
kuku --help
kuku config validate
```

kuku 将配置保存在 `config.toml` 中。使用 `kuku config show` 查看当前模型档和连接。[配置指南](../start/configuration.md)
