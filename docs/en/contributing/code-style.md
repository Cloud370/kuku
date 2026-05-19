# Code Style

## Markers

| Marker | Meaning |
|--------|---------|
| `MUST` | Hard rule. Violations are bugs. Grep-able. |
| `PREFER` | Default. Deviate only with a reason in a comment. |

## Always

- `MUST` One responsibility per module. If the description needs an "and," split it.
- `MUST` No file over 800 lines (excluding `#[cfg(test)] mod tests`). The split signal is responsibility, not line count.
- `MUST` Functions do one thing, 10–60 lines.
- `MUST` No over-defensive guards. Only protect: core invariants, system boundaries, real user experience. Do not validate for arbitrary user mistakes or imaginary scenarios.
- `MUST` No comments by default. Well-named identifiers explain WHAT. Add a comment only when the WHY is non-obvious.
- `MUST` Only four annotation tags: `// NOTE:`, `// TODO:`, `// FIXME:`, `// HACK:`. No others.
- `MUST` Enums over strings for known finite value sets. No raw `&str` where values are fixed.
- `MUST` Every `pub` item is a permanent semver commitment. `pub(crate)` is free to change.
- `MUST` No `unsafe` without a comment documenting: the invariant, why the compiler can't verify it, why safe alternatives don't work.

## Naming

- `MUST` Tool functions: `<tool_name>(args, workspace, ...)` — args and workspace always first.
- `MUST` Parsers: `<tool_name>_request(args) -> Result<Request, ToolResultEnvelope>`.
- `MUST` Renderers: `render_<what>(...)`. Never mix `format_`, `build_`, `make_`.
- `MUST` Lookups: `find_<what>` returns `Option<Snapshot>`.
- `PREFER` Constructors: `Xxx::new(...)` or `fn tool(...)`. Don't mix raw struct construction and helper functions in the same module.
- `MUST` Test names: descriptive snake_case, no abbreviations.

## Imports

- `MUST` Order: `std` → external crates → `crate::`. One blank line between groups. Alphabetical within each.
- `MUST` No wildcard imports. Exception: `use super::*` in `#[cfg(test)] mod tests`.
- `PREFER` `use std::fs;` over importing many individual `std::fs::*` items.

## Visibility

- `MUST` Default `pub(crate)` for internal items. `pub` only for public API.
- `MUST` Private helpers: no visibility modifier.
- `MUST` Function order: `pub` → `pub(crate)` → private.
- `MUST` Module declarations: `pub(crate) mod` for crate-internal; bare `mod` for truly private submodules.

## Documentation

- `MUST` `///` on every `pub` item. One sentence describing purpose.
- `MUST` `//!` module docs on public module boundary files.
- `MUST` No doc comments on `pub(crate)` or private items. Use `//` for internal notes.
- `MUST` No multi-paragraph docstrings or design rationales in code.

## Types

- `MUST` `impl` blocks immediately after struct/enum definition. Don't scatter across the file.

## Formatting

- `MUST` `rustfmt` with default settings.
- `MUST` `clippy` passes with zero warnings. `#[allow(clippy::...)]` only with a comment explaining why.
- `MUST` One blank line between items, two between sections. No trailing whitespace.

## Derive macros

- `MUST` Order: `Debug` → `Clone` → `PartialEq` → `Eq` → `Hash` → `Serialize` → `Deserialize`. Omit what isn't needed; keep the order for what remains.
- `PREFER` `derive(Default)`. Implement `Default` manually only when non-obvious and deserves a comment.

## Constants

- `MUST` `SCREAMING_SNAKE_CASE` for all `const` items.
- `MUST` Constants at top of file, after imports. Single-use constants go inside their function.
- `MUST` No magic numbers. Numbers with semantic meaning must be named.

## Match

- `MUST` Exhaustive match on enums. No `_ =>` that silently ignores new variants.
- `MUST` `if let` for single-variant; `match` for two or more. Never `match` with one arm + wildcard.

## Error handling

- `MUST` Error messages must locate the problem. `"path not found: {path}"` over `"failed"`.
- `PREFER` `unwrap()` is ok for invariants that cannot fail in practice.

## Assertions

- `MUST` `assert_eq!(expected, actual)` — expected first.
- `MUST` Add assertion message when failure reason isn't obvious from values alone.
- `MUST` No `debug_assert!` in production paths.

## Testing

- `MUST` Unit tests: `#[cfg(test)] mod tests` inside the source file.
- `MUST` Integration tests: `tests/<domain>_<aspect>.rs`. One domain boundary per file.
- `MUST` Live smoke tests: `tests/provider_live.rs`, all `#[ignore]` + env-var gated. No keys committed.
- `MUST` One test helper, one location. Shared infrastructure in `tests/common/mod.rs`.
- `PREFER` Mock data builders as simple functions, not elaborate builder patterns.
- `PREFER` Keep fixtures minimal. Extract a named helper if setup exceeds 20 lines.

## Commits

- `MUST` One commit per completed phase or logical chunk.
- `MUST` Conventional commits: `type: description` or `type(scope): description`. Types: `feat`, `fix`, `docs`, `refactor`, `test`, `chore`.
- `MUST` No amend on pushed commits. No force push to main.
- `MUST` Merge worktree branches with `--ff-only`. Linear history, no merge bubbles.
