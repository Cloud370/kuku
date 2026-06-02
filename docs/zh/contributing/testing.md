# Testing

本页定义在完成一项改动前应执行的验证工作。

## Main verification commands

在仓库根目录运行这些命令：

```bash
cargo fmt --all
cargo clippy -- -D warnings
cargo test -p kuku -p kuku-cli -p kuku-server
```

编辑过程中，如果你想快速获得反馈且不改动文件，可以使用 `cargo check`。

## Expected workflow

1. 先阅读受影响的代码和文档。
2. 编辑。
3. 工作过程中按需运行有针对性的检查。
4. 最后完整运行一次全部验证集。

## Test layout rules

- 单元测试保留在源文件内部的 `#[cfg(test)] mod tests` 中。
- 集成测试位于 `tests/` 下，每个文件只对应一个领域边界。
- 共享测试 helper 属于 `tests/common/mod.rs`。
- live provider smoke test 保持为 ignored，并由环境变量控制。不要提交 key。

## Docs changes

对于纯文档改动，仍然要通过回读页面并检查链接的栏目边界来验证你改过的页面。如果改动影响文档入口页或导航，请一起重新检查 `README.md`、`docs/index.md`、`docs/en/index.md` 和 `docs/zh/index.md`。

## What zero warnings means

`cargo clippy -- -D warnings` 是契约的一部分。如果某个 lint 必须允许，请在允许位置内联说明理由。

通用工作流见 [Development](development.md)，代码层级规则见 [Code Style](code-style.md)。
