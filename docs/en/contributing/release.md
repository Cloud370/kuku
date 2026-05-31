# Release Process

## Prerequisites

- [cargo-release](https://github.com/crate-ci/cargo-release) installed: `cargo install cargo-release`
- Push access to the repository

## Versioning

Follows [SemVer](https://semver.org/) with `v` prefix on tags.

```
v0.MINOR.PATCH
   │      │
   │      └── bug fix (patch within a feature set)
   └── new feature or module
```

- Feature release: bump MINOR, reset PATCH to 0 → `0.1.0` → `0.2.0`
- Bug fix: bump PATCH → `0.2.0` → `0.2.1`
- At 0.x, MINOR may contain breaking changes

### Pre-release tags

| Stage | Who uses it | Meaning |
|-------|-------------|---------|
| `-alpha.N` | Developers only | Feature incomplete, API may change |

Tag containing `-` is automatically marked as pre-release by CI.

During fast iteration, `-alpha.N` with incrementing N is sufficient. Once the alpha is stable, promote directly to the next stable release (e.g. `0.2.0-alpha.3` → `0.2.0`).

### When to release 1.0

When the API is stable, external users depend on it, and breaking changes should be rare. Before that, stay at 0.x.

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
