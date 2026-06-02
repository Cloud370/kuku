# Development

Use this page for the expected contributor workflow in this repository.

## Before editing

1. Read the relevant code and docs first.
2. If the task touches `docs/**`, `README.md`, translation pages, docs homepages, or docs navigation, read `docs/AGENTS.md` first.
3. Keep the English docs under `docs/en/**` as the canonical source. Chinese pages mirror the same paths later.

## Working loop

1. Make the smallest correct change.
2. Run `cargo check` as needed while editing.
3. Do not run `cargo fmt` mid-edit.
4. Verify once at the end.

## Main commands

```bash
cargo check
cargo test -p kuku -p kuku-cli -p kuku-server
cargo clippy -- -D warnings
cargo fmt --all
make build
make release-linux
```

Use the default glibc target for normal development. Use the musl release path only for release packaging.

## Documentation workflow

- Put public runtime behavior in `how-it-works/`.
- Put exact commands, formats, and configuration facts in `reference/`.
- Put internal structure and boundaries in `architecture/`.
- Put contributor workflow and repo rules in `contributing/`.
- Keep one fact fully defined in one canonical page, then link from other pages.

## Git expectations

- Use conventional commit messages.
- Keep one logical change per commit.
- Do not amend pushed commits.

## Cross-platform rule

Default to Linux, Windows 10+, and macOS. Avoid shell-specific behavior in product code. Normalize paths with `std::path::Component` rather than string slicing.

Use [Code Style](code-style.md), [Testing](testing.md), and [Release](release.md) as the concrete working rules.
