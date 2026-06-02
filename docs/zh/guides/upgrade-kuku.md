# Upgrade kuku

当前公开的 CLI 命令面并没有文档化的 `kuku update` subcommand。

## Upgrade by Reinstalling

如果你是用 Cargo 安装的：

```bash
cargo install --git https://github.com/Cloud370/kuku --force
```

如果你是通过 release script 安装的，就重新运行对应平台上的同一个脚本。

## Review Config After Upgrades

升级后检查你的配置文件：

```bash
kuku config show
kuku config validate
```

运行时在加载旧配置时，可以自动补齐缺失的 `[handoff]`、`[plugin]` 和 `[update]` 分区。

## Update Channel Settings

`config.toml` 仍然包含 update source 和 channel 设置：

```toml
[update]
source = "github"
channel = "stable"
```

参见 [Config](../reference/config.md) 和 [Update Manifest](../reference/update-manifest.md)。

## Verify the New Binary

```bash
kuku --help
```
