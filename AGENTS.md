# AGENTS.md

Agent instructions for working in this repository.

## What this is

kuku — a Rust SDK for file-native agent execution. SDK (`crates/kuku/`), CLI (`crates/kuku-cli/`), server (`crates/kuku-server/`), unified binary (`apps/kuku/`).

## Core work: read this first

Read [Core Direction](docs/en/architecture/core-direction.md) ([中文](docs/zh/architecture/core-direction.md)) before core design or implementation. It is the concise target baseline, not a claim of shipped behavior. Read one language, then only task-relevant code and sections of [Evolution](docs/en/architecture/evolution.md); do not require full-history or full-docs onboarding. Keep settled decisions in that baseline and design rationale in the owning page. Follow [Development](docs/en/contributing/development.md) for task tracking and fresh-session handoff, and [Testing](docs/en/contributing/testing.md) for acceptance evidence; do not append progress history to architecture docs. Preserve the external Prompt assets and explicit Prompt/context/provider boundaries; a core rewrite is not permission to discard them. Branch/worktree creation and implementation remain separate actions from documentation planning.

**Before the first stable release, support only the current public APIs and Session format.** Replace superseded SDK APIs, CLI commands, HTTP and streaming contracts, and Session formats directly. Update in-tree callers and tests in the same change. See [Delivery and engineering choices](docs/en/architecture/core-direction.md#delivery-and-engineering-choices).

## Project map

Full document map: [docs/en/index.md](docs/en/index.md). Key: [start](docs/en/start/index.md) · [guides](docs/en/guides/index.md) · [how-it-works](docs/en/how-it-works/index.md) · [reference](docs/en/reference/index.md) · [architecture](docs/en/architecture/index.md) · [contributing](docs/en/contributing/index.md).

## Documentation

For any work touching `docs/**`, `README.md`, translation pages, docs homepages, or docs navigation, read and follow `docs/AGENTS.md` first.

## Commands

```bash
cargo check                                            # fast compile check
python3 scripts/acceptance/run.py integration          # workspace format, lint and contracts
cargo fmt --all                                        # format before commit
make build                                             # local release build (glibc, fast)
make release-linux                                     # portable release build (musl, via cross+Docker)
```

Use the default glibc target for Linux development and tests. See [Release](docs/en/contributing/release.md) for the static musl targets and package verification.

## Code conventions

- Follow [Code Style](docs/en/contributing/code-style.md) for workspace lints, SDK contracts, and everyday defaults.
- Module dependencies follow [module-contracts.md](docs/en/architecture/module-contracts.md).

## Editing tips

- **Don't run `cargo fmt` mid-edit.** It shifts line numbers and invalidates previously read files, wasting context on re-reads. Run it once at the end.
- **`cargo check` is fine mid-edit.** It validates without modifying files. Never `cargo fix`.
- **Pipe long output through `head` or `tail`.** `cargo test | tail -30` shows failures without burning context on passing tests. Same for `grep -rn` in large repos.
- **Verify the changed behavior.** Follow [Testing](docs/en/contributing/testing.md) for required Rust integration checks, explicit feature/live-test selection, and documentation-only checks. Reuse valid results instead of rerunning solely for commit or push.
- **For collaborative TUI debugging, consider tmux.** See [Watch a TUI run together](docs/en/contributing/testing-advanced.md#watch-a-tui-run-together-optional) for an optional workflow to reproduce, edit, rebuild, and compare with a developer.

## Workflow

1. Read relevant docs and code to understand the change.
2. Edit. Run `cargo check` as needed. Do not `cargo fmt`.
3. Verify: follow [Testing](docs/en/contributing/testing.md), formatting the changed Rust scope once after editing has stopped.
4. Commit: conventional commits, one per logical chunk. No amend on pushed commits.

## Cross-platform

Default to Linux / Windows 10+ / macOS. No shell-specific behavior. Normalize paths with `std::path::Component`, never string manipulation.
