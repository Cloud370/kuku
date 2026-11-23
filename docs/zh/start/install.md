# 安装 kuku

## 从源码构建

装好 Rust 和 Cargo 后，直接安装提供 `kuku` 命令的 workspace 包：

```bash
cargo install --locked --git https://github.com/Cloud370/kuku kuku-app
kuku --help
```

默认源码构建包含 TUI、CLI 命令和 HTTP 服务。如果要用 `kuku web`，建议直接装带内嵌界面的发行包，或者[从源码构建浏览器资源](../reference/cli.md#kuku-server-与-kuku-web)。

## 安装已发布版本

在 [GitHub Releases](https://github.com/Cloud370/kuku/releases) 里找到对应版本的 `latest.json` 链接，在仓库根目录运行安装脚本：

- Linux（x86-64 / ARM64）和 macOS（Apple Silicon）：
  ```bash
  sh scripts/install.sh --source MANIFEST_URL
  ```
- Windows（x86-64）：
  ```powershell
  .\scripts\install.ps1 -Source MANIFEST_URL
  ```

不传 `--source` / `-Source` 时，安装脚本默认指向稳定版地址；体验最新开发树请用上面的 `cargo install`。[Unix 安装脚本](../../../scripts/install.sh) · [Windows 安装脚本](../../../scripts/install.ps1) · [更新清单](../reference/update-manifest.md)

安装脚本会在交互终端里显示下载大小与进度、自动重试临时失败、安装前校验 SHA-256，并在重装时直接复用匹配校验值的缓存压缩包。[下载与安装实测](../how-it-works/performance.md#下载和安装) · [低内存 ARM64 实测](../how-it-works/performance.md#低内存-arm64-设备)

在 Linux 和 macOS 上，脚本会把安装目录写入 Shell 配置。如果当前终端的 `PATH` 还没生效，先执行脚本打印出的那条命令，再启动 `kuku`。

## 下一步

确保用户主目录可写（或把 `KUKU_HOME` 指向一个可写目录），然后继续看[快速开始](quickstart.md)。
