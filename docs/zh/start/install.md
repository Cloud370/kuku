# 安装 kuku

## 构建测试版本

安装 Rust 和 Cargo 后，选择提供 `kuku` 可执行文件的 workspace 包。

```bash
cargo install --locked --git https://github.com/Cloud370/kuku kuku-app
kuku --help
```

默认构建包含 CLI 和 HTTP server。从源码构建浏览器界面的方式见 [CLI 参考](../reference/cli.md#kuku-server-和-kuku-web)。

## 安装已发布版本

安装带标签的版本时，从发布列表选取该版本的 `latest.json` 资源 URL。在仓库目录运行脚本，x86-64 Linux 或 Apple Silicon macOS 使用 `sh scripts/install.sh --source MANIFEST_URL`，x86-64 Windows 使用 `.\scripts\install.ps1 -Source MANIFEST_URL`。安装器的默认地址选择稳定版。体验当前开发版本请使用上面的 Cargo 命令。[发布列表](https://github.com/Cloud370/kuku/releases) · [Unix 安装脚本](../../../scripts/install.sh) · [Windows 安装脚本](../../../scripts/install.ps1) · [更新清单](../reference/update-manifest.md)

## 运行第一个任务

使用可写的用户目录，或将 `KUKU_HOME` 设为可写路径。在快速开始中初始化一次配置，然后选择要使用的模型服务。[快速开始](quickstart.md)
