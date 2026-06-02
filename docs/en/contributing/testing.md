# Testing

This page defines the verification work expected before finishing a change.

## Main verification commands

Run these from the repository root:

```bash
cargo fmt --all
cargo clippy -- -D warnings
cargo test -p kuku -p kuku-cli -p kuku-server
```

Use `cargo check` during editing when you want fast feedback without changing files.

## Expected workflow

1. Read the affected code and docs first.
2. Edit.
3. Run targeted checks as needed while working.
4. Run the full verification set once at the end.

## Test layout rules

- Unit tests stay in `#[cfg(test)] mod tests` inside the source file.
- Integration tests live under `tests/` with one domain boundary per file.
- Shared test helpers belong in `tests/common/mod.rs`.
- Live provider smoke tests stay ignored and env-gated. Do not commit keys.

## Docs changes

For docs-only work, still verify the pages you changed by reading them back and checking the linked section boundaries. If the change affects docs entrypoints or navigation, re-check `README.md`, `docs/index.md`, `docs/en/index.md`, and `docs/zh/index.md` together.

## What zero warnings means

`cargo clippy -- -D warnings` is part of the contract. If a lint must be allowed, document the reason inline at the allow site.

See [Development](development.md) for the general workflow and [Code Style](code-style.md) for code-level rules.
