# Release

本页记录这个仓库当前的发布规则。

## Prerequisites

- 安装 [`cargo-release`](https://github.com/crate-ci/cargo-release)：`cargo install cargo-release`
- 拥有仓库的 push 权限

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

任何包含 `-` 的 tag 都会被 CI 视为预发布。

示例演进：

```text
0.2.0-alpha.3 -> 0.2.0
```

## Release steps

1. 在 `CHANGELOG.md` 里用 `## [VERSION]` 或 `## [VERSION] - YYYY-MM-DD` 标记目标版本，省略开头的 `v`。概括用户可见的改动，有助于理解发布内容时附上实测数据。
2. 在发布候选版本上以 `platform=all` 运行完整 CI 矩阵并确认结果。[测试](testing.md#选择验证范围)
3. 运行 `cargo release <version>`。
4. 让 CI 构建产物并发布 GitHub Release。

`cargo release` 会更新 workspace 版本、创建发布 commit、创建 `v<version>` tag，并 push 两者。

首次发布时，Release 工作流把对应版本的摘要放在 GitHub Release 页面开头，再附上 GitHub 生成的变更列表。找不到对应段落时会发布生成的变更列表。重新运行工作流会保留现有发布说明。

## Build paths

- `make build` 构建正常的本地 Linux 发布 target。
- `make release-linux` 构建可移植的 musl 发布产物。

musl 路径用于发布打包，而不是日常开发。

release profile 使用 `opt-level = 2` 优先优化运行性能。构建去除符号，保留 Thin LTO、4 个 codegen unit 和 panic abort。比较二进制大小时，必须保持 target、feature 和源码版本一致。开发／测试可执行文件保留调试信息，不作为发行产物。

发行包将 Web 界面内嵌到 `kuku` 可执行文件中。用验收入口的 `interactive --archive` 模式在相应平台验证源码树外的发行包。[安装客户端验收](testing.md#安装客户端验收)

## Update channels

- `stable` 检查 `releases/latest/download/latest.json`，并跳过预发布
- `alpha` 检查最新的预发布 manifest

使用 `kuku config set update.channel alpha` 或 `kuku config set update.channel stable` 切换通道。

## When 1.0 matters

当公开 API 稳定、已有外部用户依赖它，并且破坏性变更应变得少见时，再进入 `1.0`。
