# Release Process

## Prerequisites

- [cargo-release](https://github.com/crate-ci/cargo-release) installed: `cargo install cargo-release`
- Push access to the repository

## Versioning

Follows [SemVer](https://semver.org/) with `v` prefix on tags.

- Stable: `v0.1.0`, `v0.2.0`
- Pre-release: `v0.1.0-alpha.1`, `v0.1.0-beta.1`, `v0.1.0-rc.1`

Tag containing `-` is automatically marked as pre-release by CI.

## Release Steps

1. Update `CHANGELOG.md` with the new version
2. Run `cargo release <version>` — this will:
   - Update version in all workspace Cargo.toml files
   - Create a git commit
   - Create a git tag `v<version>`
   - Push commit and tag to origin
3. CI automatically builds binaries for 3 platforms and creates a GitHub Release

## Channels

- **stable**: `kuku update` checks `releases/latest/download/latest.json` (skips pre-releases)
- **alpha**: `kuku update` checks the latest pre-release's `latest.json`

Switch with: `kuku config set update.channel alpha` (or stable)
