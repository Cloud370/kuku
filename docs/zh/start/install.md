# Install

## Prerequisites

- 如果你从源码安装，需要先准备 Rust 和 `cargo`。
- 至少为一个已配置 provider 准备好 API key。
- 需要可写的 home 目录，或者把 `KUKU_HOME` 设为一个可写路径。

## Install with Cargo

```bash
cargo install --git https://github.com/Cloud370/kuku
```

这会从仓库安装 `kuku` binary。

## Install with the Release Script

当前文档也提供了 release 安装脚本：

- Linux 和 macOS：`curl -fsSL https://kuku.run/install.sh | sh`
- Windows PowerShell：`irm https://kuku.run/install.ps1 | iex`

这些脚本使用与 [Update Manifest](../reference/update-manifest.md) 中描述相同的更新清单。

## Verify the Install

```bash
kuku --help
kuku init
```

`kuku init` 会创建默认运行时目录结构，并在 `config.toml` 还不存在时写入它。

## Next

1. 在 shell 或 `config.toml` 中设置 provider 凭证。
2. 查看 [Configuration](configuration.md)。
3. 运行 [Quickstart](quickstart.md)。
