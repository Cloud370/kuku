# Core Direction — Read First

## Status and reading contract

This is the canonical target for the rewrite, not shipped behavior. Existing guides and code describe the current release.

Read this page first, then only task-relevant code and [Evolution](evolution.md). Keep this page under 100 lines and replace superseded decisions instead of appending discussion history. English is canonical; keep the Chinese structure aligned. Documentation approval does not start implementation.

## Product and design test

kuku is an **embeddable, file-native Rust Agent SDK with a usable default Harness**. A small, complete execution runtime supports interactive work, independent subagents, Teams, and event-driven long-running work. Embedded callers use the public SDK directly. For the interactive product, the server owns execution through that SDK and the CLI, TUI and WebUI use a common client protocol. [Interactive clients](interactive-clients.md)

Give reliability, low resource use, and low human/model cognitive cost equal weight. A mechanism needs a concrete job and one owner. The default Harness supplies working prompts, tools, model selection, and delegation; callers need not assemble an agent platform. Custom Harnesses choose recipients, context, scheduling, and work completion through explicit SDK operations, without replacing persistence, authorization, or execution.

The default Harness follows the user’s intended stage of work, including exploration, drafts and delivery. Evaluation callers own benchmark budgets, scoring and unattended scheduling. A normal turn follows the conversation and tool results to completion. A caller with an execution budget can enforce it independently.

Leave the generation limit to the provider when its protocol allows omission. An Agent or host can set an explicit ceiling. Presets for protocols that require a limit supply it in configuration.

## Preserve from kuku

- **Keep behavioral Prompt text in versioned assets/templates**, outside Rust execution branches and tool implementations. Structured runtime facts are not themselves Prompt templates.
- **Keep Prompt rendering, context assembly, and provider adaptation separate.** Assets own wording; context owns selection, ordering, and recorded sources; providers declare model capabilities and translate prepared requests. Persist what entered model context.
- Preserve native Skills, configurable Presets/Tiers, file-backed facts, inspectable assembly, useful provider/tool implementations, and relevant tests. Rewrite coupled ownership and execution paths; the existing [Prompt Assembly](prompt-assembly.md) is evidence, not a requirement to keep its replay algorithm.

## Persistent objects and execution records

| Concept | Target meaning |
|---|---|
| Session | Persistent history and execution facts for a set of Agents; the unit of opening, recovery, resource ownership, and data retention |
| Agent | A Session-local stable address with its own context, current configuration, pending input, and resumable execution state |
| Turn | A recorded round of admitted work for one Agent; ending a Turn does not establish business-task success |
| Step | One logical model request and the tool calls caused by its response |
| ToolCall | One action with requested, authorized, dispatched, and settled facts |
| Job | A handle for an existing background execution and output, not another executor or copied result |

Session and Agent are the core persistent objects. Use **flat, readable Agent addresses**, for example `main`, `reviewer`, and `agent-3`, unique and never reassigned within a Session. Titles may change; suspension, reopening, and later work keep the same identity. Humans, model tools, and runtime use that address; cross-Session references add the Session identity. Do not add a hidden parallel Agent UUID/alias system. Example spellings are not frozen API names.

Record creation relationships separately. A default main Agent has no intrinsic authority over all others; creation ancestry is not task ownership, communication permission, or resource lifetime. Subagents normally remain in the same Session. Independent Agents may communicate when authorized; a parent Turn ending does not cancel descendants. Task boards, Team membership/roles, and long-lived assistant identities belong to Harnesses. Work may span Agents/Sessions; accept external correlation without requiring a Task object for ordinary SDK use.

## One execution path

- Agent creation and input submission support caller idempotency: retrying the same command returns its original durable receipt; conflicting reuse is rejected. Receipt, model admission, and completion are distinct facts. Input carries its actual source, target, and correlation; seeing context grants no new authority.
- Delivery, admission, and activation are separate. Default sending wakes an idle recipient; a custom Harness may enqueue without activation and control later advancement. Running Agents admit input only at safe boundaries. Preserve pending input, batch admission, and coalesce wakeups. Explicit stops need authorized continuation.
- One Agent has at most one advancing execution chain; different Agents can run concurrently. The Session writer serializes commits; model/tool I/O does not hold the writer. Inbox and status are derived from the committed log. Runtime receipts and routing do not require model-written acknowledgements or a model classifying every message.
- Prepare each Step once, binding admitted input, resolved model/settings, tool definitions, and context. Technical retries reuse it under one retry owner. Later input/configuration applies at later boundaries; obsolete callbacks cannot mutate a replacement execution.
- Accept a model's tool calls as a batch. Run independent work concurrently, order conflicts safely, bind authorization to final arguments and the actual tool definition, and preserve call/result pairing and partial failures.
- Runtime progresses independently of observers. Expose submission, configuration, explicit advancement/control, query, and resumable observation through the same public SDK. Losing or slowing a subscriber cannot lose committed results or cause unbounded buffering.

## Model choice and independent context

Support **OpenAI Chat Completions, OpenAI Responses, and Anthropic Messages** as distinct wire protocols through the same runtime. Each must support the core streaming, tool-use, and multi-turn execution path. Resolve capabilities for the actual configured endpoint/model; sharing a protocol does not imply identical reasoning, caching, service options, or server-side state. Keep protocol encoding, connection authentication, and model capabilities separate. Vendor-specific handling stays at the provider boundary without adding another executor or changing the core context model.

Use one resolver for the SDK, user interfaces, main Agents, and subagents. Presets define behavior/tools/initial defaults; Tiers are reusable model-route/settings defaults. Select an exact configured connection and model, never the first connection matching a protocol. Explicit Agent choices override defaults and never mutate other Agents or process-wide defaults.

Adapters describe and validate available reasoning and service options. These are independent choices: enabling supported acceleration does not implicitly change the model or reasoning. Do not impose a universal `fast/ultrafast` ladder. Unsupported explicit choices fail visibly. Record requested settings, resolved settings sent, and provider-reported settings when available; unavailable confirmation stays unknown.

New Agents start with **independent context by default**, without forking the parent's transcript. A Harness explicitly chooses task material and references. Reusing an Agent preserves its own accumulated context. Resource defaults do not imply context inheritance; a route change must not silently carry incompatible provider-specific settings.

## Durable facts and cache continuity

- **Committed files are authoritative; memory is disposable acceleration.** Start with one append-only Session log, one owning writer, and separate large-output files. Recover with bounded streaming reads and append incrementally without rescanning history. Small rebuildable indexes may support identity, idempotency, and lookup; add checkpoints or physical segmentation only for measured needs.
- Define durable barriers before acknowledging input, dispatching effects, and confirming results. Save referenced output before confirming its reference. Recovery reconstructs facts, not suspended async stacks. Unknown effects are not automatically repeated; persistence failure stops dependent work. Record necessary facts, not every internal transition.
- Stream tool output with bounded buffers and explicit quotas. First model admission uses a rule-based structured summary or bounded fallback with readable references and visible filtering/incompleteness. Preserve that exact view. Short calls and Jobs share execution/output; later Job completion is not a second result for the original ToolCall. No hidden command rewriting, global shell hooks, or default summarizer LLM.
- Provide one bounded output reader for summary/range/search/incremental access; UI reads do not consume model output. Retain references for their promised lifetime, not OS temporary-file expiry. Do not introduce a generic Artifact platform.
- **Submitted provider-visible prefixes stay immutable within a context segment.** Store admitted content once and reference it, rather than copying full history per Step. Never rerender history from today's templates/config/files or retroactively prune admitted results. Capacity or incompatible changes use a committed successor segment and recorded handoff. Cache reuse remains a first-class goal, not a reason to force shared Agent histories or promise provider cache retention.

## Waiting and resource lifetime

Represent resumable waiting at explicit safe boundaries, including questions or authorization before dispatch. Persist the pending request, target action, and reply correlation; release unneeded execution capacity. Resume only the matching live request after validating its current conditions. Never interpret an unrelated message as approval or transparently resume arbitrary in-flight processes.

Distinguish waiting, canceling current execution, and closing live resources. Cancellation stops new dispatch and cleans up owned work; asynchronous close awaits cleanup and durable writes and reports failures. Reopening restores pending facts for explicit Harness scheduling. Timers, external event intake, cross-Session assistant routing, and outgoing delivery policies belong to the Harness/host.

Bound model requests, tool processes, inbox admission, output, and observer queues separately. Dormant Agents need no polling model calls, dedicated OS threads/processes, or permanent full-history copies. Share immutable assets and suitable clients; keep only needed active context. Waiting for a person consumes no model-execution slot. Measure local memory/I/O separately from cache-aware model cost and user-visible latency.

## Workspace, trust, and scope

Workspace actions should return bounded model evidence and machine-readable execution facts for hosts. Explicitly named Session values should survive reopening with on-demand access to their content. The proposed [workspace execution design](workspace-execution.md) covers scripting, retention, resource costs and acceptance.

A workspace is an ordinary directory. Agents share it by default; callers may choose another directory. Core behavior is identical with and without Git. Worktrees, commits, and merges remain explicit choices; file conflict checks use observed content/version. The SDK does not seize process cwd, environment, signals, global logging, or unrelated processes.

Target trusted callers/single-user use; the host supplies strong OS isolation when needed. The default local entry explicitly uses a trusted-local policy; embedded hosts can supply their own policy. A host authorization boundary receives the real caller, final action/arguments, and directory, and allows, denies, or defers it. Error/cancellation is never permission. No mandatory rule DSL, role hierarchy, approval platform, or model-based safety classifier is required.

Do not start with all-plugin composition, hot-swappable loops, generic event middleware, multiple storage backends, distributed writers, built-in Team/task platforms, or workflow DSLs. Reference systems supply evidence for individual contracts, not a package layout or product to reproduce.

## Delivery and engineering choices

kuku is in testing and has no established user base. **Before the first stable release, support only the current public APIs and Session format.** Replace superseded SDK APIs, CLI commands, HTTP and streaming contracts, addresses, and Session formats directly. Omit compatibility shims, old readers, and migrations for previous builds. Update in-tree callers and tests in the same change. Preserve user files and verification evidence. The SDK stages and acceptance conditions live in [Evolution](evolution.md#delivery-plan-public-sdk-execution-slice).

Deliver real single-Agent work through the public SDK and a thin CLI first, then early independent Agents using different providers, then durable waiting and small external-orchestration examples. Choose a few mature benchmark cases with their original verifiers, plus focused contract tests. Do not build a benchmark platform or demand an architecture-proof phase before useful execution.

**Rebuild the development workflow alongside the runtime.** A fresh coding session must be able to pick up scoped work, reproduce validation, report evidence, and leave a reliable handoff without prior chat history. Keep progress out of architecture prose, preserve private work records, and keep tracking and test tooling lightweight. [Development](../contributing/development.md) owns this workflow; it is not a runtime dependency.

Ownership, identity, admission, configuration scope, durable effects, and resource limits are the target commitments. Exact Rust signatures, event schemas, numeric limits, parsers, selected cases, and checkpoint choices are engineering decisions within them. Change this baseline explicitly if evidence requires a different commitment; do not infer a new direction from one reference implementation.
