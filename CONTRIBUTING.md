# Contributing to kuku

[中文版](CONTRIBUTING.zh.md)

Contributions that are likely to be accepted:

- Bug fixes
- Support for new LLM providers
- Environment-specific fixes (Windows, macOS, Linux)
- Missing standard behavior
- Documentation improvements
- Test coverage improvements

For new features, significant refactors, or API changes, please open an issue to discuss with maintainers before writing code.

Chinese (中文) is preferred for issues and pull requests. English is also welcome. Use [Conventional Commits](docs/en/contributing/development.md#git-and-platform-rules) for commit messages.

## Code Style

Follow the [code style](docs/en/contributing/code-style.md) and format the edited Rust files after editing.

## Development

```bash
# Build
cargo build

# Run workspace contracts
python3 scripts/acceptance/run.py contract

# Run the CLI
cargo run -- run say hello
```

## Pull Requests

- **Every PR must be linked to an issue.** If no issue exists, open one first. Unlinked PRs will be closed.
- Keep PRs focused — one logical change per PR
- Follow [Testing](docs/en/contributing/testing.md) to verify the change before pushing.
- Follow the PR template

PRs that ignore these guidelines may be closed.
