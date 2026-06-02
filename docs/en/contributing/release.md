# Release

This page keeps the current release rules for the repository.

## Prerequisites

- Install [`cargo-release`](https://github.com/crate-ci/cargo-release): `cargo install cargo-release`
- Have push access to the repository

## Versioning

Releases follow SemVer and use a `v` prefix on git tags.

```text
v0.MINOR.PATCH
```

- Feature release: bump `MINOR`, reset `PATCH` to `0`
- Bug fix release: bump `PATCH`
- While the project is in `0.x`, `MINOR` releases may still contain breaking changes

## Pre-releases

Use `-alpha.N` for developer-facing pre-releases when a feature set is not yet stable.

Any tag containing `-` is treated as a pre-release by CI.

Example progression:

```text
0.2.0-alpha.3 -> 0.2.0
```

## Release steps

1. Update `CHANGELOG.md` for the target version.
2. Run `cargo release <version>`.
3. Let CI build artifacts and publish the GitHub Release.

`cargo release` updates workspace versions, creates the release commit, creates the `v<version>` tag, and pushes both.

## Build paths

- `make build` builds the normal local Linux release target.
- `make release-linux` builds the portable musl release artifact.

Use the musl path for release packaging, not normal development.

## Update channels

- `stable` checks `releases/latest/download/latest.json` and skips pre-releases
- `alpha` checks the latest pre-release manifest

Switch channels with `kuku config set update.channel alpha` or `kuku config set update.channel stable`.

## When 1.0 matters

Move to `1.0` when the public API is stable, external users depend on it, and breaking changes should become rare.
