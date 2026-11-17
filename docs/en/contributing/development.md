# Development

This page owns the contributor workflow, task handoff, and development-workflow requirements for the rewrite. [Core Direction](../architecture/core-direction.md) owns product decisions; [Evolution](../architecture/evolution.md) owns delivery stages.

## Before editing

1. Confirm the intended checkout, branch, current commit, working-tree changes, and active task. Preserve work already in progress.
2. Read the task's goal, scope, dependencies, and acceptance conditions, then the relevant code and owning docs. Core work starts with Core Direction; do not require a full-history or full-documentation read.
3. For documentation changes, read `docs/AGENTS.md`. English under `docs/en/**` is canonical; update the corresponding Chinese pages in the same change.

## Task records and progress

Use one selected work tracker as the source of current task state. A task is a reviewable result, not a tool call or every source file. Start with the existing delivery stages and split only for independently verifiable work, dependencies, or separate ownership.

Each active task records its intended result, scope and owner, dependencies, acceptance conditions, current state, code reference, evidence location, blocker if any, and next action. Omit inapplicable fields and link to existing context instead of copying it. Distinguish planned, active, blocked, and verified work; written code alone is not verified delivery.

Update the same task when work starts, a meaningful blocker or decision changes it, a result is verified, or ownership is handed off. Do not post every command, poll continuously, or estimate an unsupported completion percentage. Keep historical detail in commits, PRs, and run artifacts. Architecture pages carry neither progress tables nor dated journals.

Maintainers use a private GitHub Project for internal work. [Draft items](https://docs.github.com/en/issues/planning-and-tracking-with-projects/managing-items-in-your-project/adding-items-to-your-project) exist in the Project rather than in a repository. A private Project does not make a linked public Issue or PR private; check the [visibility rules](https://docs.github.com/en/issues/planning-and-tracking-with-projects/managing-your-project/managing-visibility-of-your-projects) before publication. Keep internal discussion and private evidence in an appropriately restricted location.

The tracker is an asynchronous coordination surface, not a dependency of builds, tests, or the SDK. Batch updates at meaningful boundaries through existing tools. If an update fails, disclose that it is unsynchronized rather than claiming it was published. Do not build a tracking service, duplicate local status board, or Project automation before a concrete need exists.

## Fresh-session handoff

Record the worktree, branch and commit for the assigned task. Use `git worktree list`, `git status --short --branch` and `git rev-parse HEAD` to check them. Read the checkout's `AGENTS.md`, the task and the design it implements. For interactive client work, use the [client plan](../architecture/interactive-clients.md).

The task locator and handoff must let a fresh session identify the current branch/commit, any uncommitted work, active processes or owners, verified results and their limits, unresolved work, and next action. Confirm the actual checkout and artifacts before continuing. Do not require the previous chat or an old Agent to remain available, and do not start duplicate writers.

The local locator for this rewrite is ignored `benchmarks/rewrite-state.json`. It points to the selected private Project and task identifiers; task status stays in that Project. Preserve the locator and follow its references rather than creating a second board. It may be absent in another checkout, so carry the tracker reference in the assigned task. Keep reproducible commands in the maintained [acceptance selector](testing.md#verification-selection), and link to bounded run records from the selected task instead of writing parallel handoff pages.

Credential loading and local evidence locations belong to [Testing](testing.md#credentials-and-run-artifacts). Handoff references the configuration; it never copies keys or private request payloads.

## Working loop

1. State the observable result and the smallest acceptance check before implementation. Update the owning design first when an agreed commitment changes; ordinary engineering choices do not require a new decision document.
2. Implement one coherent slice, reusing existing tools and helpers. Run `cargo check` or focused behavior checks when they resolve a concrete uncertainty.
3. After edits to the affected Rust files have stopped, format the changed scope once and follow [Testing](testing.md) for required integration checks. Reuse passing evidence unless changes invalidate it.
4. Review the actual diff against the task and evidence. Fix failures, or record the precise missing validation; a skipped live test is not a passing live test.
5. Commit the logical result and update its task. A PR states the problem, resulting behavior, relevant checks actually run, and material limitations. Reference unfinished parent work without claiming the whole rewrite is complete.

## Workflow deliverables for the rewrite

Use these checks to verify changes to the SDK and its hosts. The [runtime stages](../architecture/evolution.md#delivery-plan-public-sdk-execution-slice) explain the contracts behind them.

| Delivery boundary | Workflow acceptance |
|---|---|
| First usable SDK/CLI slice | A fresh session can find one task and reproduce its useful result through the public entrypoint; one maintained test entrypoint or small case manifest selects relevant checks and produces bounded, inspectable evidence |
| Independent Agents and protocols | The same verification path supports deterministic protocol fixtures and budgeted live calls; the selected models, capabilities, usage, and missing coverage are explicit |
| Waiting and recovery | Fault/reopen checks and resource measurements remain repeatable; a fresh session can resume unfinished development from the task, checkout, and evidence without reconstructing chat history |

Adapt existing test helpers, scripts, PR guidance, and CI as their behavior changes. Remove obsolete checks and close gaps such as silently unselected feature-gated tests; do not merely rewrite command lists in prose. CI carries the supported platform matrix, while local work uses focused feedback and the required integration checks.

Keep the implementation small: no service requirement, broad workflow framework, mandatory stack of skills, per-task report files, or gate for every editorial rule. Important invariants deserve automated checks when those checks catch concrete failures. Long-lived rationale belongs with the owning design; task history stays with the task.

## Main commands

```bash
cargo check
python3 scripts/acceptance/run.py contract
python3 scripts/acceptance/run.py protocol
python3 scripts/acceptance/run.py integration
cargo fmt --all -- --check
make build
make release-linux
```

The selector records what actually ran; a selected check is not proof of unselected live or platform coverage. [Testing](testing.md) owns feature selection, original benchmark environments/verifiers, cost reservations, and live-call prerequisites. Use the default glibc target for normal development and the musl release path only for release packaging.

## Git and platform rules

Use conventional commits with one logical change per commit. Do not amend pushed commits. Follow [Code Style](code-style.md) for integration rules and [Release](release.md) for publishing.

Default to Linux, Windows 10+, and macOS. Avoid shell-specific behavior in product code. Normalize paths with `std::path::Component` rather than string slicing.

The workflow borrows separation of current documentation, task state, and evidence from [DSH's documentation rules](https://github.com/deepseek-ai/deepseek-harness/blob/39163dd30f703a79079e2d2446545cff3c453738/docs/AGENTS.md) and [testing policy](https://github.com/deepseek-ai/deepseek-harness/blob/39163dd30f703a79079e2d2446545cff3c453738/docs/testing.md). Its extensive note lifecycle and Project automation are not prerequisites for kuku.
