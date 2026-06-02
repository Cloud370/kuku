# AGENTS.md

Agent instructions for working in this repository.

## What this is

kuku — a Rust SDK for file-native agent execution. SDK (`crates/kuku/`), CLI (`crates/kuku-cli/`), server (`crates/kuku-server/`), unified binary (`apps/kuku/`).

## Project map

Full document map: [docs/en/index.md](docs/en/index.md). Key: [start](docs/en/start/index.md) · [guides](docs/en/guides/index.md) · [how-it-works](docs/en/how-it-works/index.md) · [reference](docs/en/reference/index.md) · [architecture](docs/en/architecture/index.md) · [contributing](docs/en/contributing/index.md).

## Documentation

For any work touching `docs/**`, `README.md`, translation pages, docs homepages, or docs navigation, read and follow `docs/AGENTS.md` first.

## Commands

```bash
cargo check                                            # fast compile check
cargo test -p kuku -p kuku-cli -p kuku-server                    # all tests
cargo clippy -- -D warnings                            # zero warnings required
cargo fmt --all                                        # format before commit
make build                                             # local release build (glibc, fast)
make release-linux                                     # portable release build (musl, via cross+Docker)
```

Release builds use `x86_64-unknown-linux-musl` via `cross` (fully static, no glibc dependency). Use the default glibc target for development and tests; musl only for release packaging.

## Code conventions

- `MUST` / `PREFER` rules in [code-style.md](docs/en/contributing/code-style.md) are the contract.
- No file over 1000 lines. Enums over strings. No wildcard imports. No comments by default.
- Module dependencies follow [module-contracts.md](docs/en/architecture/module-contracts.md).

## Editing tips

- **Don't run `cargo fmt` mid-edit.** It shifts line numbers and invalidates previously read files, wasting context on re-reads. Run it once at the end.
- **`cargo check` is fine mid-edit.** It validates without modifying files. Never `cargo fix`.
- **Pipe long output through `head` or `tail`.** `cargo test | tail -30` shows failures without burning context on passing tests. Same for `grep -rn` in large repos.
- **Verify once at the end.** `cargo fmt --all && cargo clippy -- -D warnings && cargo test -p kuku -p kuku-cli -p kuku-server`, then commit.

## Workflow

1. Read relevant docs and code to understand the change.
2. Edit. Run `cargo check` as needed. Do not `cargo fmt`.
3. Verify: `cargo fmt --all && cargo clippy -- -D warnings && cargo test -p kuku -p kuku-cli -p kuku-server`.
4. Commit: conventional commits, one per logical chunk. No amend on pushed commits.

## Cross-platform

Default to Linux / Windows 10+ / macOS. No shell-specific behavior. Normalize paths with `std::path::Component`, never string manipulation.
