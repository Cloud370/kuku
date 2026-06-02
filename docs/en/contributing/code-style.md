# Code Style

This page is the concrete style contract for repository code.

## Markers

| Marker | Meaning |
|--------|---------|
| `MUST` | Hard rule. Violations are bugs. |
| `PREFER` | Default. Deviate only with a reason. |

## General rules

- `MUST` One responsibility per module. If the description needs an "and", split it.
- `MUST` No file over 1000 lines, excluding `#[cfg(test)] mod tests`.
- `MUST` Functions do one thing and usually stay in the 10-60 line range.
- `MUST` No over-defensive guards. Protect core invariants, system boundaries, and real user-facing failure modes.
- `MUST` No comments by default. Add one only when the reason is non-obvious.
- `MUST` Only four annotation tags: `// NOTE:`, `// TODO:`, `// FIXME:`, `// HACK:`.
- `MUST` Use enums instead of strings for finite known value sets.
- `MUST` Treat every `pub` item as a semver commitment.
- `MUST` Document any `unsafe` block with the invariant, the missing compiler proof, and why safe alternatives are not enough.

## Naming

- `MUST` Tool functions follow `<tool_name>(args, workspace, ...)` with args and workspace first.
- `MUST` Tool request parsers follow `<tool_name>_request(args) -> Result<Request, ToolResultEnvelope>`.
- `MUST` Renderers use `render_<what>(...)`.
- `MUST` Lookups named `find_<what>` return `Option<Snapshot>`.
- `PREFER` Constructors use `Xxx::new(...)` or `fn tool(...)` consistently within a module.
- `MUST` Test names are descriptive snake_case.

## Imports and visibility

- `MUST` Import order is `std`, then external crates, then `crate::`, with one blank line between groups.
- `MUST` No wildcard imports, except `use super::*` in unit tests.
- `PREFER` `use std::fs;` over importing many individual `std::fs::*` items.
- `MUST` Default to `pub(crate)` for internal items.
- `MUST` Private helpers have no visibility modifier.
- `MUST` Function order is `pub`, then `pub(crate)`, then private.
- `MUST` Use `pub(crate) mod` for crate-internal modules and bare `mod` for private submodules.

## Documentation in code

- `MUST` Add `///` to every `pub` item with one sentence describing its purpose.
- `MUST` Add `//!` module docs to public module boundary files.
- `MUST` Do not add doc comments to `pub(crate)` or private items.
- `MUST` Keep docstrings short. Do not place long design rationale in code comments.

## Formatting and types

- `MUST` Keep `impl` blocks immediately after the related type definition.
- `MUST` Use `rustfmt` with default settings.
- `MUST` Keep `clippy` at zero warnings. Any `#[allow(clippy::...)]` needs a short reason.
- `MUST` Use one blank line between items and no trailing whitespace.

## Derives and constants

- `MUST` Derive order is `Debug`, `Clone`, `PartialEq`, `Eq`, `Hash`, `Serialize`, `Deserialize`.
- `PREFER` `derive(Default)` instead of manual `Default` when the default is obvious.
- `MUST` Use `SCREAMING_SNAKE_CASE` for all constants.
- `MUST` Place constants near the top of the file after imports, unless a constant is single-use and clearer inside one function.
- `MUST` Name semantically meaningful numbers instead of leaving magic numbers inline.

## Control flow and errors

- `MUST` Match exhaustively on enums. Do not hide future variants behind `_ =>`.
- `MUST` Use `if let` for a single-variant check and `match` for two or more branches.
- `MUST` Error messages locate the problem.
- `PREFER` `unwrap()` only for invariants that cannot fail in practice.

## Assertions and tests

- `MUST` Write `assert_eq!(expected, actual)` with expected first.
- `MUST` Add an assertion message when the failure reason would otherwise be unclear.
- `MUST` Do not use `debug_assert!` in production paths.
- `MUST` Keep unit tests in `#[cfg(test)] mod tests` inside the source file.
- `MUST` Keep integration tests in `tests/<domain>_<aspect>.rs` with one domain boundary per file.
- `MUST` Keep live provider smoke tests in `tests/provider_live.rs`, gated by env vars and `#[ignore]`.
- `MUST` Put shared test infrastructure in `tests/common/mod.rs`.
- `PREFER` Use simple helper functions instead of elaborate test builders.

## Commits

- `MUST` Keep one completed logical chunk per commit.
- `MUST` Use conventional commit messages: `type: description` or `type(scope): description`.
- `MUST` Do not amend pushed commits.
- `MUST` Merge worktree branches with `--ff-only`.
