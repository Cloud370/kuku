# kuku

> A fast, low-memory, file-native agent runtime and Rust SDK.

kuku stays light enough that a 371 MiB RAM ARM64 board can open and continue a 1,000+ turn conversation in the terminal while paging through older history. [Performance measurements](docs/en/how-it-works/performance.md#low-memory-arm64-device)

[English Docs](docs/en/index.md) | [中文文档](docs/zh/index.md) | [Docs Portal](docs/index.md)

[Contributing](CONTRIBUTING.md) | [中文贡献指南](CONTRIBUTING.zh.md)

> [!WARNING]
> kuku is still pre-1.0. Until the first stable release, compatibility covers only the current SDK, CLI, server API, and Session format. Updates may break existing integrations or make older Sessions unreadable.

## Install

Copy the block for your platform to install published release [`v0.1.0-alpha.4`](https://github.com/Cloud370/kuku/releases/tag/v0.1.0-alpha.4). Follow that release's guide after installing it. [Release guide](https://github.com/Cloud370/kuku/tree/v0.1.0-alpha.4#quick-start) · [中文安装](docs/zh/start/install.md#安装已发布版本)

### Linux and macOS

Linux x86-64, Linux ARM64, and Apple Silicon Macs.

```sh
curl -fsSL https://raw.githubusercontent.com/Cloud370/kuku/v0.1.0-alpha.4/scripts/install.sh | sh -s -- \
  --source https://github.com/Cloud370/kuku/releases/download/v0.1.0-alpha.4/latest.json \
  --version 0.1.0-alpha.4
```

### Windows

Run in PowerShell on Windows x86-64.

```powershell
$kukuTag = 'v0.1.0-alpha.4'
$kukuInstaller = Invoke-RestMethod "https://raw.githubusercontent.com/Cloud370/kuku/$kukuTag/scripts/install.ps1"
& ([scriptblock]::Create($kukuInstaller)) -Source "https://github.com/Cloud370/kuku/releases/download/$kukuTag/latest.json" -Version ($kukuTag.Substring(1))
```

Open a new terminal after installation. [Build from source](docs/en/start/install.md#build-from-source)

## Quick Start

The following guides use the binary built from this source tree. [Build from source](docs/en/start/install.md#build-from-source)

Open a terminal in your project directory and run kuku.

```bash
kuku
```

Choose a connection and a model. Type `Inspect the files in this directory` and press Enter. You can also ask `How do I use kuku?` once the model is configured. [Terminal quickstart](docs/en/start/quickstart.md)

## Documentation

- [Start in the terminal](docs/en/start/quickstart.md) to set up a model and send a message.
- [Run from a script](docs/en/guides/run-a-task.md) to configure and submit a headless task, then read its result.
- [Embed the Rust SDK](docs/en/reference/sdk.md#first-request) to open a Session and make a first request.

## Repo Map

- SDK: [`crates/kuku/`](crates/kuku/)
- CLI: [`crates/kuku-cli/`](crates/kuku-cli/)
- Server: [`crates/kuku-server/`](crates/kuku-server/)
- HTTP client and terminal UI: [`crates/kuku-client/`](crates/kuku-client/), [`crates/kuku-tui/`](crates/kuku-tui/)
- Unified binary: [`apps/kuku/`](apps/kuku/)

## License

Licensed under either of:

- MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)
- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
