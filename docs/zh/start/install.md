# 安装 kuku

## 从源码构建

装好 Rust 和 Cargo 后，从源码检出的根目录运行以下命令，安装与本文档对应的二进制。

```bash
cargo install --locked --path apps/kuku
kuku --help
```

这个 workspace 包提供 TUI、脚本命令和 HTTP 服务。要直接从开发分支构建，可运行 `cargo install --locked --git https://github.com/Cloud370/kuku kuku-app`。

## 安装已发布版本

复制对应平台的命令，安装已发布的 [`v0.1.0-alpha.4`](https://github.com/Cloud370/kuku/releases/tag/v0.1.0-alpha.4)。安装后参阅对应版本的文档。[该版本的快速开始](https://github.com/Cloud370/kuku/blob/v0.1.0-alpha.4/docs/zh/start/quickstart.md)

**Linux（x86-64 或 ARM64）和 macOS（Apple Silicon）**

```sh
curl -fsSL https://raw.githubusercontent.com/Cloud370/kuku/v0.1.0-alpha.4/scripts/install.sh | sh -s -- \
  --source https://github.com/Cloud370/kuku/releases/download/v0.1.0-alpha.4/latest.json \
  --version 0.1.0-alpha.4
```

**Windows（x86-64）**

在 PowerShell 中运行这组命令。

```powershell
$kukuTag = 'v0.1.0-alpha.4'
$kukuInstaller = Invoke-RestMethod "https://raw.githubusercontent.com/Cloud370/kuku/$kukuTag/scripts/install.ps1"
& ([scriptblock]::Create($kukuInstaller)) -Source "https://github.com/Cloud370/kuku/releases/download/$kukuTag/latest.json" -Version ($kukuTag.Substring(1))
```

安装后打开新终端，运行 `kuku`。

要安装其他版本，先在 [GitHub Releases](https://github.com/Cloud370/kuku/releases) 找到对应版本的 `latest.json` 链接，再从仓库根目录运行对应的安装脚本。

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

使用从当前源码构建的二进制，按[快速开始](quickstart.md)完成配置。确保用户主目录可写，或把 `KUKU_HOME` 指向可写目录。
