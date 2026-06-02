# Code Style

本页是仓库代码的具体风格契约。

## Markers

| Marker | Meaning |
|--------|---------|
| `MUST` | 硬性规则。违反就是 bug。 |
| `PREFER` | 默认规则。只有在有理由时才偏离。 |

## General rules

- `MUST` 每个模块只有一个职责。如果描述里需要出现 “and”，就拆分。
- `MUST` 任何文件都不能超过 1000 行，`#[cfg(test)] mod tests` 除外。
- `MUST` 函数只做一件事，通常保持在 10-60 行范围内。
- `MUST` 不要过度防御。保护核心不变量、系统边界和真实面向用户的失败模式。
- `MUST` 默认不写注释。只有在原因并不明显时才加一条。
- `MUST` 只允许四种注释标签：`// NOTE:`、`// TODO:`、`// FIXME:`、`// HACK:`。
- `MUST` 对有限且已知的值集合使用 enum，而不是字符串。
- `MUST` 把每个 `pub` 项都当作 semver 承诺。
- `MUST` 为任何 `unsafe` block 记录不变量、编译器缺失的证明，以及为什么安全替代方案不够。

## Naming

- `MUST` Tool 函数遵循 `<tool_name>(args, workspace, ...)`，并把 args 和 workspace 放在前面。
- `MUST` Tool 请求解析器遵循 `<tool_name>_request(args) -> Result<Request, ToolResultEnvelope>`。
- `MUST` 渲染器使用 `render_<what>(...)`。
- `MUST` 名为 `find_<what>` 的查找函数返回 `Option<Snapshot>`。
- `PREFER` 构造器在模块内一致地使用 `Xxx::new(...)` 或 `fn tool(...)`。
- `MUST` 测试名使用具有描述性的 snake_case。

## Imports and visibility

- `MUST` import 顺序是 `std`、外部 crate、`crate::`，组与组之间留一个空行。
- `MUST` 禁止通配符 import，单元测试中的 `use super::*` 除外。
- `PREFER` 使用 `use std::fs;`，而不是 import 很多个单独的 `std::fs::*` 项。
- `MUST` 内部项默认使用 `pub(crate)`。
- `MUST` 私有 helper 不写可见性修饰符。
- `MUST` 函数顺序是 `pub`、`pub(crate)`、私有。
- `MUST` 对 crate 内部模块使用 `pub(crate) mod`，对私有子模块使用裸 `mod`。

## Documentation in code

- `MUST` 给每个 `pub` 项添加 `///`，用一句话说明用途。
- `MUST` 在公开模块边界文件中添加 `//!` 模块文档。
- `MUST` 不要给 `pub(crate)` 或私有项添加文档注释。
- `MUST` 保持 docstring 简短。不要把很长的设计理由放进代码注释。

## Formatting and types

- `MUST` 把 `impl` block 紧跟在相关类型定义之后。
- `MUST` 使用默认设置的 `rustfmt`。
- `MUST` 让 `clippy` 保持零 warning。任何 `#[allow(clippy::...)]` 都需要简短理由。
- `MUST` 条目之间只留一个空行，不要有行尾空白。

## Derives and constants

- `MUST` derive 顺序是 `Debug`、`Clone`、`PartialEq`、`Eq`、`Hash`、`Serialize`、`Deserialize`。
- `PREFER` 当默认值很明显时，优先 `derive(Default)` 而不是手写 `Default`。
- `MUST` 所有常量都使用 `SCREAMING_SNAKE_CASE`。
- `MUST` 常量应放在文件顶部 import 之后，除非该常量只用一次且写在函数内部更清晰。
- `MUST` 对有语义意义的数字命名，不要把 magic number 直接写在行内。

## Control flow and errors

- `MUST` 对 enum 做穷尽匹配。不要用 `_ =>` 隐藏未来变体。
- `MUST` 单变体检查使用 `if let`，两个或更多分支使用 `match`。
- `MUST` 错误消息要能定位问题。
- `PREFER` 只有在实践中不可能失败的不变量场景才使用 `unwrap()`。

## Assertions and tests

- `MUST` 按 `assert_eq!(expected, actual)` 书写，并把 expected 放在前面。
- `MUST` 如果失败原因否则不清楚，就添加断言消息。
- `MUST` 不要在生产路径中使用 `debug_assert!`。
- `MUST` 单元测试保留在源文件内部的 `#[cfg(test)] mod tests` 中。
- `MUST` 集成测试放在 `tests/<domain>_<aspect>.rs`，每个文件只对应一个领域边界。
- `MUST` live provider smoke test 保留在 `tests/provider_live.rs`，并用环境变量和 `#[ignore]` 控制。
- `MUST` 共享测试基础设施放在 `tests/common/mod.rs`。
- `PREFER` 使用简单 helper 函数，而不是复杂的测试 builder。

## Commits

- `MUST` 每个 commit 只包含一个已完成的逻辑块。
- `MUST` 使用 conventional commit message：`type: description` 或 `type(scope): description`。
- `MUST` 不要 amend 已经 push 的 commit。
- `MUST` 用 `--ff-only` 合并 worktree 分支。
