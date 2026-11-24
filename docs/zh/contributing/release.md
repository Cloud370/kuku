# Release

本页记录这个仓库当前的发布规则。

## Prerequisites

发布需要有权限推送发行 tag 的仓库账号。

## Versioning

发布遵循 SemVer，并在 git tag 上使用 `v` 前缀。

```text
v0.MINOR.PATCH
```

- 功能发布：提升 `MINOR`，并把 `PATCH` 重置为 `0`
- Bug 修复发布：提升 `PATCH`
- 在项目仍处于 `0.x` 阶段时，`MINOR` 发布仍可能包含破坏性变更

## Pre-releases

当一组功能尚未稳定时，使用 `-alpha.N` 作为面向开发者的预发布后缀。

带 SemVer 预发布后缀的 tag 会被 CI 视为预发布。

示例演进：

```text
0.2.0-alpha.3 -> 0.2.0
```

## Release steps

1. 确定版本号和待发布的提交。[发布公告](#发布公告)
2. 演练时，在候选 ref 上手动运行 Release 工作流，将 `tag` 留空。把目标版本填入 `preview-version` 可验收二进制版本和更新清单。从该次运行下载安装包和 `release-metadata` 产物核对。
3. 在候选提交上创建带注释的 `v<version>` tag 并推送该 tag。统一二进制从 tag 读取发行版本。
4. 工作流通过后，核对 GitHub Release 的下载包、校验值、更新清单和预发布状态。

Release 在 Linux、macOS 和 Windows 上运行全工作区合约，同时构建安装包。它直接验收构建好的包，并在原生 Linux ARM64 runner 上验证 ARM64 包。同一提交的这些检查通过后才公开发布。演练的结果保存在工作流产物里。

## 发布公告

在 `CHANGELOG.md` 的 `## [VERSION]` 或 `## [VERSION] - YYYY-MM-DD` 标题下准备中文用户公告。Release 会在公告后附上自动生成的 PR 清单和贡献者致谢，并优先与同通道已公开的上一版比较，附上完整源码差异的链接。

请贡献者或 Agent 写公告时，可以直接给出版本和候选提交。

> 为 `<版本>` 写一份面向用户的中文发布公告。对比上一版和 `<候选 SHA>`，核对已合并的 PR、源码和验收证据。按使用场景归纳可感知的变化、必要的升级操作和已知限制，把摘要写入 `CHANGELOG.md` 对应版本。只引用这次候选已经验证的实测数据，提交明细由发布流程自动补齐。

## Build paths

- `make build` 构建正常的本地 Linux 发布 target。
- `make release-linux` 构建可移植的 x86-64 musl 发布产物。

Linux 发布包含 x86-64 和 ARM64 的静态 musl 包。CI 构建两种架构，并在原生 Linux ARM64 runner 上验证 ARM64 包。日常开发使用宿主机 target，需要测试设备时可在本地交叉编译 `aarch64-unknown-linux-musl`。

release profile 使用 `opt-level = 2` 优先优化运行性能。构建去除符号，保留 Thin LTO、4 个 codegen unit 和 panic abort。比较二进制大小时，必须保持 target、feature 和源码版本一致。开发／测试可执行文件保留调试信息，不作为发行产物。

发行包提供终端客户端、服务端 API 和两份项目许可证。需要可选的 Web 界面时，可启用 `embedded-web-assets` 构建。用验收入口的 `interactive --archive` 模式在相应平台验证源码树外的发行包。[构建 Web 界面](../reference/cli.md#kuku-server-与-kuku-web) · [安装客户端验收](testing.md#安装客户端验收)

## Update channels

- `stable` 检查 `releases/latest/download/latest.json`，并跳过预发布
- `alpha` 检查最新的预发布 manifest

使用 `kuku config set update.channel alpha` 或 `kuku config set update.channel stable` 切换通道。

## When 1.0 matters

当公开 API 稳定、已有外部用户依赖它，并且破坏性变更应变得少见时，再进入 `1.0`。
