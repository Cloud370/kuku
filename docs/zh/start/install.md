# 安装

## 准备工作

- 如果你从源码安装，需要 Rust 和 `cargo`。
- 至少为一个已配置的 provider 准备好 API key。
- 可写的 home 目录，或将 `KUKU_HOME` 设置为可写路径。

## 通过 Cargo 安装

```bash
cargo install --git https://github.com/Cloud370/kuku
```

这会从仓库安装 `kuku` 可执行文件。

## 通过 Release 脚本安装

当前文档也定义了 release 安装脚本：

- Linux 和 macOS：`curl -fsSL https://kuku.run/install.sh | sh`
- Windows PowerShell：`irm https://kuku.run/install.ps1 | iex`

这些脚本使用与 [Update Manifest](../reference/update-manifest.md) 中所述相同的更新清单。

## 验证安装

```bash
kuku --help
kuku init
```

`kuku init` 创建默认运行时布局，若 `config.toml` 尚不存在则写入它。

## 下一步

1. 在 shell 或 `config.toml` 中设置 provider 凭证。
2. 查阅 [Configuration](configuration.md)。
3. 运行 [Quickstart](quickstart.md)。
