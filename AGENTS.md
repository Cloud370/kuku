# AGENTS.md

Agent instructions for working in this repository.

## What this is

kuku — a Rust SDK for file-native agent execution. SDK (`crates/kuku/`), CLI (`crates/kuku-cli/`), server (`crates/kuku-server/`), unified binary (`apps/kuku/`).

## Project map

Full document map: [docs/en/README.md](docs/en/README.md). Key: [direction](docs/en/core/direction.md) · [architecture](docs/en/core/architecture.md) · [agent-loop](docs/en/core/agent-loop.md) · [glossary](docs/en/glossary.md) · [code-style](docs/en/contributing/code-style.md) · [modules](docs/en/contributing/modules.md).

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
- Module dependencies follow [modules.md](docs/en/contributing/modules.md).

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
