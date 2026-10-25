# Core Evolution — Implementation Notes

## Purpose

[Core Direction](core-direction.md) is the product decision baseline. This page is optional engineering reference, not another onboarding document or a fixed implementation checklist. Current code behavior remains in [Sessions](../how-it-works/sessions.md), [Prompt Assembly](prompt-assembly.md), and [Module Contracts](module-contracts.md).

Checkpoint 2026-10-25: documentation only. Start implementation with the public-SDK vertical slice, not UI or a framework. Record the active batch's usable result and evidence here when work actually begins. Old-version maintenance, API compatibility, and old-session migration are not delivery requirements; existing data is not deleted by that decision.

## Preserve the good boundaries

Keep external Prompt assets, explicit layers, and provenance. Prompt rendering owns wording, context assembly selects/orders material, and providers translate prepared requests. Do not move behavioral prose into Rust execution branches or let provider adapters select history. Existing [Prompt Assembly](prompt-assembly.md) is useful evidence, not an obligation to retain its replay algorithm.

Rewrite coupled ownership where needed: Session events/writer, Agent input/runtime, Step preparation, tool batches/processes, delegation/messages, and authorization placement. Reuse working provider protocols, tool bodies, Skills, configuration loading, and tests. The public SDK must work without a daemon, private-module access, or global process takeover; async shutdown reports cleanup/flush failures rather than relying only on `Drop`.

## Minimal persistence and recovery

Use one writer with cross-process exclusion and stale-owner fencing. Define durable input acceptance, dispatch, and result boundaries. A requested tool is not a started tool; an effect without a settled result is unknown, not automatically retryable. Reject committed-history corruption, recover only identifiable incomplete tails, and stop dependent work after persistence failure.

Start with streaming open/replay and incremental append. If measured resume latency needs it, add one validated checkpoint tied to a durable log prefix, safely published and disposable. If browsing needs it, add a small rebuildable offset index. Neither requires a database, segmentation, compression, or multiple storage backends. Recovery restores facts, not suspended async stacks.

## Prepared requests and handoff

Bind effective model/settings, tools, rendered content, and adapter representation to the Step. Retry in one designated layer using that prepared input. Record requested/effective selection; do not silently change models, reasoning, or service modes. New messages/configuration wait for a new boundary.

Accepted content and references must reconstruct what the model actually saw, not today's templates or mutable files. A handoff records predecessor, selected facts/provenance, and effective configuration. Commit a usable successor before switching; failure preserves the previous explainable context. Prefix-incompatible header/tool/route changes need a boundary. Safety or user-requested changes are not delayed to keep caches warm.

## Tool batches and output

Receive all calls from one model response together. Schedule genuinely independent work concurrently, serialize conflicting work, preserve deterministic call/result pairing, and report partial failures. The actual action is validated/authorized before dispatch; editing rejects stale expected content instead of overwriting concurrent changes. Prefer using existing multi-call support over new wrapper tools for every workflow.

Foreground and background commands share one execution and output path. The original ToolCall returns once: a completed result or a Job handle. Later Job settlement is a separate fact/notification. Cancellation stops dispatch and settles or fences owned work; a surviving process/output file does not prove success.

Stream stdout/stderr with bounded buffers, parser state, diagnostics, and views. Persist lifecycle and exact model-visible views, not every progress chunk or duplicated raw logs. Start with a generic preview and a few explicit build/test rules. Do not rewrite commands implicitly; uncertain compound commands and parser errors fall back visibly. Execution supplies exit status; summaries mark filtering and incomplete capture and expose readable ranges/watermarks.

Define output quotas, disk-full behavior, retention, and secure access before promising complete capture. Limits must stop or drain/discard explicitly, not deadlock or hide loss. Referenced output does not silently expire as an OS temp file. Deletion accounts for owned active work; shared-artifact retention and compression are not initial requirements.

## Workspace and authorization

Use ordinary filesystem directories, with no Git discovery required for opening a Session or selecting its data location. Worktree creation, commits, diffs, and merges remain explicit optional operations. Core change records come from execution facts, not mandatory `git diff`. Shared-workspace safety is not transactional isolation: concurrent semantic conflicts remain possible even with guarded edits.

The permission proposal is defined in Core Direction; do not turn it into multiple middleware layers. Bind decisions to the real caller and final action, not model-supplied claims or a pre-transformation command. A host that needs user interaction may await it inside the authorization boundary; errors/cancellation cannot fall through to execution. Any remembered grants or elaborate policy belong to a concrete host need, not an assumed kernel framework. Approval checks do not substitute for an OS sandbox.

## Messages and resource ownership

Use Session-scoped canonical Agent paths without a second identity system. Messages share the fact log; preserve sender/reply attribution and deduplicate admission. Running recipients admit at Step boundaries; idle delivery wakes by default, coalesced to prevent duplicate drivers. Stopped/closed recipients require authorized continuation. Basic budgets stop automatic wake chains visibly without dropping work.

Sibling communication does not grant cancellation, configuration, or human-approval powers. Ending a parent Turn is not subtree cancellation; inspectable sibling traffic need not all enter the parent's context. Runtime receipts/status need no model call. Each model request, process, Job, and subscription has an owner; classify provider failure, invalid request, tool failure, unknown effects, cancellation, and persistence failure without a generic state-machine framework.

## Evidence to select for each batch

- Public SDK use in equivalent Git and non-Git directories; no private interfaces, implicit repository setup, or required server.
- Durable acceptance, dispatch/result crash windows, competing writers, disk-full, damaged tails, unknown effects, and late callbacks.
- Provider-visible prefix equality after restart; template/config changes do not rerender history; failed handoff stays explainable.
- Safe batch concurrency, canceled authorization/execution, bounded partial output, consistent range reads, and one original tool result.
- Same human/model addresses, sibling authority, idle wake, deduplicated admission, bounded continuation, and slow/dropped observers.
- Append/replay/range-read/context latency and peak memory at increasing physical history/output sizes; add accelerators against measured budgets only.

Use the public SDK with an actual supported model and real file/command tools early, in a controlled workspace. Verify useful task completion and inspect resulting files, execution records, and model-visible results; a greeting or scripted tool sequence is not sufficient. The CLI uses the same public SDK. Do not make a mock framework or an architecture-proof phase a prerequisite for implementation; if real-model access is unavailable, identify that external blocker and leave end-to-end validation explicitly incomplete.

Use focused mocks or fault injection where a boundary needs repeatable evidence: interrupted responses, result-write failure, or cancel/completion races. Replace only the relevant dependency while retaining the actual execution, authorization, and storage path. Assert outcomes and invariants rather than exact model wording or an assumed tool order unless that order is itself required. Tests protect contracts, not define the whole product; neither a passing mock nor one successful real run proves every failure boundary.

Measure useful work per model call separately from local I/O/RAM; cumulative tokens do not imply huge disk history. Keep private session identities, task text, and raw logs out of shared docs. Schema/API names, thresholds, parsers, and batch boundaries remain adjustable; product decisions belong in Core Direction, not another plan.

## Optional reference baseline

DeepSeek Harness revision `22ddee3792402c3e70ec5f86a63c3e3e895a2f60`; the installed runtime may differ. No comparative runtime benchmark is claimed. Read only for the relevant boundary.

- [Loop vocabulary](https://github.com/deepseek-ai/deepseek-harness/blob/22ddee3792402c3e70ec5f86a63c3e3e895a2f60/docs/glossary.md) and [Agent loop](https://github.com/deepseek-ai/deepseek-harness/blob/22ddee3792402c3e70ec5f86a63c3e3e895a2f60/packages/core/agent-loop/README.md): request preparation and lifecycle.
- [Recovery](https://github.com/deepseek-ai/deepseek-harness/blob/22ddee3792402c3e70ec5f86a63c3e3e895a2f60/packages/core/session/src/repair.ts) and [Jobs](https://github.com/deepseek-ai/deepseek-harness/blob/22ddee3792402c3e70ec5f86a63c3e3e895a2f60/packages/jobs/jobs/README.md): unknown outcomes, ownership, and process-local limits.
- [Messaging](https://github.com/deepseek-ai/deepseek-harness/blob/22ddee3792402c3e70ec5f86a63c3e3e895a2f60/packages/subagent/tool-subagent-control/README.md): DSH wakes idle recipients but limits ordinary messaging to adjacent parent/child Agents; kuku also plans permitted sibling communication.
- [Spill](https://github.com/deepseek-ai/deepseek-harness/blob/22ddee3792402c3e70ec5f86a63c3e3e895a2f60/packages/spill/spill-policy/README.md) versus [historical pruning](https://github.com/deepseek-ai/deepseek-harness/blob/22ddee3792402c3e70ec5f86a63c3e3e895a2f60/packages/compaction/compaction-tool-result-pruner/README.md): borrow bounded first admission, not retroactive prefix changes.
