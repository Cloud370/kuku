# Development

本页用于说明这个仓库里贡献者应遵循的工作流。

## Before editing

1. 先阅读相关代码和文档。
2. 如果任务涉及 `docs/**`、`README.md`、翻译页面、文档主页或文档导航，先阅读 `docs/AGENTS.md`。
3. 把 `docs/en/**` 下的英文文档视为规范来源。中文页面稍后按相同路径镜像。

## Working loop

1. 做最小且正确的改动。
2. 编辑过程中按需运行 `cargo check`。
3. 不要在编辑中途运行 `cargo fmt`。
4. 在最后统一验证一次。

## Main commands

```bash
cargo check
cargo test -p kuku -p kuku-cli -p kuku-server
cargo clippy -- -D warnings
cargo fmt --all
make build
make release-linux
```

日常开发使用默认的 glibc target。仅在发布打包时使用 musl 发布路径。

## Documentation workflow

- 面向公开运行时行为的内容放在 `how-it-works/`。
- 精确命令、格式和配置事实放在 `reference/`。
- 内部结构和边界放在 `architecture/`。
- 贡献者工作流和仓库规则放在 `contributing/`。
- 一条事实只在一个规范页面里完整定义，其他页面用链接引用。

## Git expectations

- 使用 conventional commit message。
- 每个 commit 只包含一个逻辑变更。
- 不要 amend 已经 push 的 commit。

## Cross-platform rule

默认面向 Linux、Windows 10+ 和 macOS。在产品代码中避免 shell 特定行为。使用 `std::path::Component` 规范化路径，而不是字符串切片。

把 [Code Style](code-style.md)、[Testing](testing.md) 和 [Release](release.md) 作为具体工作规则。
