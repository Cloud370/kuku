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

1. Update `CHANGELOG.md` with a `## [VERSION]` or `## [VERSION] - YYYY-MM-DD` heading for the target version, without the leading `v`. Summarize user-visible changes and include measured results when they help explain the release.
2. Run the full CI matrix with `platform=all` on the release candidate and verify the result. [Testing](testing.md#verification-selection)
3. Run `cargo release <version>`.
4. Let CI build artifacts and publish the GitHub Release.

`cargo release` updates workspace versions, creates the release commit, creates the `v<version>` tag, and pushes both.

The Release workflow attaches the packages and manifest before publishing. On first publication, it puts that version's summary at the top of the GitHub Release and appends GitHub's generated changelog. A release without a matching section gets the generated changelog. A rerun can finish an existing draft and preserves its written description.

## Build paths

- `make build` builds the normal local Linux release target.
- `make release-linux` builds the portable musl release artifact.

Use the musl path for release packaging, not normal development.

The release profile uses `opt-level = 2` for runtime performance, with stripped symbols, Thin LTO, four codegen units and aborting panics. Compare binary sizes using the same target, features and source revision. Development/test executables retain debug information and are not distribution artifacts.

Release archives bundle the Web UI in the `kuku` executable. Use the acceptance selector's `interactive --archive` mode on the corresponding platform to verify an archive outside the source tree. [Installed client acceptance](testing.md#installed-client-acceptance)

## Update channels

- `stable` checks `releases/latest/download/latest.json` and skips pre-releases
- `alpha` checks the latest pre-release manifest

Switch channels with `kuku config set update.channel alpha` or `kuku config set update.channel stable`.

## When 1.0 matters

Move to `1.0` when the public API is stable, external users depend on it, and breaking changes should become rare.
