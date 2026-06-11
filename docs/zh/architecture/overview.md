# Architecture Overview

这个栏目面向维护者。它说明 crate 边界和运行时归属，而不是面向最终用户的行为。

## High-level split

- `crates/kuku/` 是 SDK。
- `crates/kuku-cli/` 提供 CLI 命令实现。
- `crates/kuku-server/` 提供 HTTP server。
- `apps/kuku/` 构建统一的发布二进制。

SDK 负责运行时事实、事件持久化、上下文重建、provider 调用、Tool 分发和权限决策。host 负责展示、传输和交互。

## SDK shape

```text
crates/kuku/src/
|- query/
|- context/
|- provider/
|- tool/
|- permission/
|- session/
|- event/
|- prompt/
|- skill/
|- plugin/
|- config/
|- conversation/
|- agent/
|- notice/
|- util/
|- wire.rs
`- error.rs
```

## Runtime invariants

- Session 真相在磁盘上。
- `events.jsonl` 只追加，不回写。
- 每次 model 调用前都会重建上下文。
- provider adapter 负责协议格式转换，但不拥有运行时状态。
- 权限检查发生在运行时里，而不是 host UI 中。

## Data layout

所有 kuku 状态都位于 `$KUKU_HOME` 下，包括配置、`Memory`、项目策略和 Session 目录。

## Reading path

如果你需要面向用户的 host 边界，先看 [Host Apps](host-apps.md)；如果你需要请求构造，先看 [Prompt Assembly](prompt-assembly.md)；如果你需要内部归属规则，先看 [Module Contracts](module-contracts.md)。

对于公开行为，请使用 [How It Works](../how-it-works/index.md)，不要使用本栏目。
