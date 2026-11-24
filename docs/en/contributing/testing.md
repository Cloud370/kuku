# Testing

Use the [acceptance selector](../../../scripts/acceptance/run.py) from the checkout under test. It requires Python 3.10+ and records the selected checks and their evidence.

## Verification selection

Use controlled provider fixtures for routine development and CI. Use [live provider checks](testing-advanced.md#live-protocol-acceptance) when the behavior under test depends on a real upstream, such as authentication, routing, usage reporting, or model behavior on a task.

| Change | Check |
| --- | --- |
| Documentation only | [Documentation checks](#documentation-checks) |
| Rust behavior and public contracts | `python3 scripts/acceptance/run.py contract` |
| Provider encoding, streaming, replay, or settings | `python3 scripts/acceptance/run.py protocol` |
| Rust changes ready for integration | `python3 scripts/acceptance/run.py integration` |
| TUI input, Session navigation, service discovery, embedded browser, or packaging | [Installed client acceptance](#installed-client-acceptance) |
| Acceptance runner or fixture tooling | `python3 -m unittest discover -s scripts/acceptance -p 'test_*.py'` |

During editing, run the smallest check that exercises the changed behavior. For a runtime integration batch, run `integration` once after formatting the edited Rust files. It runs the read-only format check, workspace/all-target Clippy with warnings denied, and `contract`. The contract selection enables `kuku/test_support` and includes the gated runtime, client, and TUI suites. `protocol` runs controlled provider fixtures without paid calls.

To focus on the feature-gated provider and query integration suites, use the dedicated Cargo targets.

```bash
cargo test -p kuku --features test_support --test provider_integration --test query_runtime
```

Before merging, confirm the selected ready-PR checks pass for the intended revision. The [CI plan](../../../scripts/acceptance/ci_plan.py) selects native and installed scenarios from the PR change set. The [release workflow](release.md) runs its own contracts and archive acceptance for the candidate.

## Installed client acceptance

Run the controlled scenarios against the current source build. Use a built binary to repeat a selected scenario.

```bash
python3 scripts/acceptance/run.py interactive
python3 scripts/acceptance/run.py interactive --binary PATH_TO_BINARY --cases statusline
```

These checks use a local provider and run the real client and service over PTY or ConPTY. For native smoke, browser handoff, release archives, package upgrades, desktop input, and replay, see the [installed client recipes](testing-advanced.md#installed-client-recipes).

## Test ownership and evidence

Keep unit tests close to their source and integration tests under `tests/` by domain. Reuse the existing fixtures. Put each invariant at the layer that owns it. For example, provider tests own retry timing and Session tests own interruption and recovery.

Look in `benchmarks/acceptance/<run-id>/result.json` for the source revision, features, selected commands, exit statuses, and evidence paths. Read the bounded command log, the failure summary when present, or `installed-interactive.log` for terminal scenarios. Build time reports, when collected, are under `target/cargo-timings/`. Keep a separate Cargo target for each worktree and verify the installed binary's revision before using it as evidence.

Report what actually ran and whether it passed, failed, or was skipped. Separate deterministic coverage, native platform results, and hands-on experience. Verify tool effects and cleanup through durable records and fixture assertions. Share bounded evidence and keep credentials, private requests, and Session contents out of public logs.

[Advanced acceptance](testing-advanced.md) explains how to select benchmarks and resource measurements and how to run authorized live checks.

## Documentation checks

Read the changed pages back, verify local links and anchors, and check that the English and Chinese pages agree. Run `git diff --check`. For an entry or navigation change, also check the [README](../../../README.md), [docs portal](../../index.md), and [English](../index.md) and [Chinese](../../zh/index.md) indexes.
