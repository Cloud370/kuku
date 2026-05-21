# Contributing to kuku

Contributions that are likely to be accepted:

- Bug fixes
- Support for new LLM providers
- Environment-specific fixes (Windows, macOS, Linux)
- Missing standard behavior
- Documentation improvements
- Test coverage improvements

For new features, significant refactors, or API changes, please open an issue to discuss with maintainers before writing code.

## Code Style

kuku enforces code style automatically. Before committing, run:

```bash
cargo fmt --all
cargo clippy -- -D warnings
```

See [docs/en/contributing/code-style.md](docs/en/contributing/code-style.md) for the full convention.

## Development

```bash
# Build
cargo build

# Run tests
cargo test -p kuku -p kuku-cli -p kuku-server

# Run the CLI
cargo run -- run say hello
```

## Pull Requests

- **Every PR must be linked to an issue.** If no issue exists, open one first. Unlinked PRs will be closed.
- Keep PRs focused — one logical change per PR
- Run `cargo fmt --all && cargo clippy -- -D warnings && cargo test -p kuku -p kuku-cli -p kuku-server` before pushing
- Follow the PR template

PRs that ignore these guidelines may be closed.
