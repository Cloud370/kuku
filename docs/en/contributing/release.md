# Release

This page keeps the current release rules for the repository.

## Prerequisites

To publish, use an account with permission to push release tags.

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

CI treats a tag with a SemVer pre-release suffix as a pre-release.

Example progression:

```text
0.2.0-alpha.3 -> 0.2.0
```

## Release steps

1. Choose the release version and the exact commit to publish. [Release announcement](#release-announcement)
2. For a rehearsal, dispatch Release on the candidate ref with `tag` empty. Set `preview-version` to the intended version to test its binary version and update manifest. Download the packages and `release-metadata` artifact to review them.
3. Create an annotated `v<version>` tag at the candidate commit and push that tag. The unified binary reads its release version from the tag.
4. Check the GitHub Release after the workflow passes. Verify the expected artifacts, checksums, update manifest and pre-release status.

Release runs the workspace contracts on Linux, macOS and Windows in parallel with packaging. It runs installed-client acceptance on the built archives and verifies the Linux ARM64 archive on a native runner. Publication starts after those checks pass for the same commit. A rehearsal keeps the results in workflow artifacts.

## Release announcement

Prepare a Chinese user-facing announcement under `## [VERSION]` or `## [VERSION] - YYYY-MM-DD` in `CHANGELOG.md`. Release follows it with the generated PR list and contributor credits. It prefers the previous published ancestor from the same update channel and links to the complete source comparison.

Ask a contributor or an agent to review the complete range from the previous published version to the candidate using the merged PRs, code and acceptance evidence. Use separate sections for new features, improvements, bug fixes and upgrade notes where relevant. Write about what users can do and include measured results only when the release candidate has evidence for them. The generated PR details appear in an expandable section.

To preview the announcement on its own, run Release with `tag` empty, set `preview-version`, and enable `notes-only`. Download the `release-notes` artifact.

## Build paths

- `make build` builds the normal local Linux release target.
- `make release-linux` builds the portable x86-64 musl release artifact.

Linux releases ship static musl archives for x86-64 and ARM64. CI builds both and verifies the ARM64 archive on a native Linux ARM64 runner. Use the host target for normal development. Cross-compile `aarch64-unknown-linux-musl` locally when you need a device build.

The release profile uses `opt-level = 2` for runtime performance, with stripped symbols, Thin LTO, four codegen units and aborting panics. Compare binary sizes using the same target, features and source revision. Development/test executables retain debug information and are not distribution artifacts.

Release archives ship the terminal client, server API, and both project license files. Build with `embedded-web-assets` when you want the optional Web UI. Use the acceptance selector's `interactive --archive` mode on the corresponding platform to verify an archive outside the source tree. [Web UI build](../reference/cli.md#kuku-server-and-kuku-web) · [Installed client acceptance](testing.md#installed-client-acceptance)

## Update channels

- `stable` checks `releases/latest/download/latest.json` and skips pre-releases
- `alpha` checks the latest pre-release manifest

Switch channels with `kuku config set update.channel alpha` or `kuku config set update.channel stable`.

## When 1.0 matters

Move to `1.0` when the public API is stable, external users depend on it, and breaking changes should become rare.
