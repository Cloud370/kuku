# 升级 kuku

## 更新可执行文件

通过 Cargo 安装测试版时，重新安装仓库当前版本提供的统一二进制。

```bash
cargo install --locked --git https://github.com/Cloud370/kuku --force kuku-app
```

使用带标签的发行版时，选择目标版本的清单，并使用对应平台的安装脚本。[安装已发布版本](../start/install.md#安装已发布版本)

## 核对配置

```bash
kuku --help
kuku config validate
```

kuku 将配置保存在 `config.toml` 中。使用 `kuku config show` 查看当前模型档和连接。[配置指南](../start/configuration.md)
