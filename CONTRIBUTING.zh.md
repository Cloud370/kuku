# 参与 kuku 开发

[English](CONTRIBUTING.md)

欢迎以下贡献。

- 修复 Bug
- 支持新的 LLM 提供方
- 修复 Windows、macOS 或 Linux 上的问题
- 补齐缺失的标准行为
- 改进文档
- 增加测试覆盖

新增功能、较大重构或 API 变更，请先开 Issue 和维护者讨论，再开始编写代码。

Issue 和 PR 推荐使用中文，英文也欢迎。Commit 信息遵循 [Conventional Commits](docs/zh/contributing/development.md#git-与平台规则) 规范。

## 代码风格

kuku 会自动检查代码风格。提交前请运行以下命令。

```bash
cargo fmt --all
cargo clippy -- -D warnings
```

完整规范见[代码风格](docs/zh/contributing/code-style.md)。

## 开发

```bash
# 构建
cargo build

# 运行测试
cargo test -p kuku -p kuku-cli -p kuku-server

# 运行 CLI
cargo run -- run say hello
```

## Pull Request

- **每个 PR 都必须关联一个 Issue。** 如果还没有对应的 Issue，请先创建。未关联 Issue 的 PR 会被关闭。
- 让每个 PR 聚焦一个逻辑完整的改动。
- 推送前运行 `cargo fmt --all && cargo clippy -- -D warnings && cargo test -p kuku -p kuku-cli -p kuku-server`。
- 遵循 PR 模板。

未遵循这些规则的 PR 可能被关闭。
