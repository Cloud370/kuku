# Core Direction — Read First

## Status and reading contract

Design baseline: 2026-10-25. This page is the canonical concise statement of the target direction, not a claim that the rewrite exists. It supersedes earlier planning suggestions of independent child Sessions, opt-in idle wake, and mandatory storage indexes/checkpoints. Current implementation behavior remains in the existing guides and code.

For core work, read this page, then only the relevant code and [Evolution](evolution.md) section. Do not reconstruct decisions from chat history or read the whole documentation tree. Keep this page under 100 lines; replace superseded decisions rather than appending discussion transcripts. English is canonical; keep the Chinese mirror aligned.

## Product and design test

kuku is an **embeddable, file-native Rust Agent SDK**, not an application with internal methods exported afterwards, nor a component kit requiring users to assemble an Agent. CLI, server, and UI are thin hosts of the same usable runtime. No mandatory daemon, HTTP server, subprocess CLI, or plugin container is required to embed it.

The goal is a lightweight work harness: low overhead, reliable long-running work, and concepts shared by humans, models, and implementation. Make concepts intuitive and visible, not hidden behind modes. Before adding a mechanism, ask what real work requires it, why existing concepts cannot express it, and whether it reduces total complexity.

## Preserve from kuku

- **Prompt text remains external to Rust execution logic.** Keep model-facing behavioral instructions in versioned assets/templates; code loads, renders, and composes them instead of scattering instruction prose through branches and tool implementations. Structured runtime data is not itself a Prompt template.
- **Prompt rendering and context assembly stay separate.** Prompt assets define wording; context assembly explicitly selects and orders policy, Agent identity, tool guidance, memory, Skills, accepted history, and new input with provenance; providers translate the prepared request into protocol form.
- Preserve stable layers and inspectable assembly, file-backed facts, native Skills, configurable presets/tiers, host separation, and useful provider/tool implementations and tests. Rewrite ownership and execution boundaries, not everything that already works. See [Prompt Assembly](prompt-assembly.md) for the existing implementation, not a requirement to keep every old replay algorithm.

## One identity and execution model

| Concept | Target meaning |
|---|---|
| Session | One durable work session containing the main Agent and delegated work |
| Agent | A Session-local address: `/root`, `/root/agent1`, `/root/agent1/agent1`; owns its dialogue and execution |
| Turn | One round of admitted input and work for that address |
| Step | One logical model request plus tools caused by its response |
| ToolCall | One action with requested, authorized, dispatched, and settled facts |
| Job | A handle for background work, not another executor or output copy |

Humans, model tools, and runtime use the same Agent path. Allocate once and never reuse it for different work; titles may change. Qualify with Session identity only across Sessions. Do not add a parallel hidden Agent UUID/alias system. Paths express delegation, not filesystem locations or authority. Conversation is the address's dialogue, Attempt is a Step's concrete model attempt, and a context segment is a stable-prefix request series: no extra identity hierarchy is needed for them.

Subagents stay in the Session, with independent execution/configuration; do not turn them into independent Sessions or a Team framework. Delegation is a tree, permitted communication can cross siblings, and waits are explicit. Humans and Agents share observable semantics, not human-only approval powers.

## Input, execution, and configuration

- Sending durably queues a message. A running recipient admits it at the next safe Step boundary; an **idle recipient starts a Turn by default**. An explicitly stopped/closed recipient needs authorized continuation. Sending does not wait for a reply; runtime receipts and completion are not model-written chatter.
- Batch pending inputs and coalesce wakeups. Preserve unadmitted messages across failures; inbox is a log-derived view, not a second store. Guide concise collaboration with short instructions; enforce ownership and basic execution budgets in code, not another model call judging every message.
- Prepare each Step once: admitted input, effective model selection, tool definitions, and context. Technical retries reuse that prepared input; later input/configuration waits for a later boundary. Retry ownership is explicit, not multiplied across layers.
- Preset defines working instructions/tools/defaults; Tier selects a resource route/defaults; explicit model, reasoning effort, and service settings override according to one documented resolution rule. Record requested and effective choices. Unsupported explicit choices fail clearly; never silently lower reasoning or ignore a provider-specific fast/service setting.
- Accept all calls from one model response as a batch. Schedule independent work concurrently and conflicting work safely in order without an extra model round trip per call. Authorize final arguments and bind tool definitions; grouping does not bypass validation or hide partial failure.

## Facts, output, and immutable prefixes

- **Committed files are authoritative; memory is disposable acceleration.** Start with one append-only Session log, one owning writer, and separately stored large output. Open with bounded streaming replay; append incrementally, never rescan history per write. Add one validated rebuildable checkpoint or small offset index only when measured latency requires it; physical segmentation is not a starting requirement.
- Define acknowledgement/durability barriers before confirming input, dispatching effects, and confirming results. Record necessary execution facts, not every internal transition. Recovery reconstructs facts, not suspended async stacks; unknown effects are never automatically retried. Write failure stops dependent work; cancel means resource cleanup, not just a changed label.
- Tool output streams to Session-owned files using bounded buffers. First admission returns a rule-based structured summary or bounded fallback, explicitly marked filtered/incomplete with a readable reference. Retain exactly what the model saw. No global shell hook, hidden command rewriting, or default summarizer LLM. Short calls and Jobs share one execution/output; later Job completion is not a second result for the original ToolCall.
- Use one bounded output-reading entry point for summary/range/search/incremental access. UI reads do not consume model output. Set result, output-file, and Session-output budgets; disclose loss and preserve references for their promised lifetime, not OS temporary-file expiry. Do not build a generic Artifact platform for this.
- **Submitted provider-visible prefixes are immutable within a context segment.** Filter before first admission only. Do not rerender accepted content from today's templates/config/files or retroactively prune tool results. Capacity or incompatible request-header changes require an explicit successor segment with recorded handoff; old history stays intact. Stable prefixes improve eligibility, not guarantee provider cache retention.

## Workspace and SDK boundaries

A workspace is an ordinary directory, not a Git repository. Main and child Agents share it by default; callers may explicitly assign another directory. Core execution, persistence, file tools, and change recording must work the same with or without Git. Git commands and worktrees are optional tools/user choices, never implicit setup, isolation, or merge machinery. File conflict checks use observed content/version, not Git availability.

Expose usable defaults plus explicit model/tool/host adapters, submission, observation, querying, cancellation, and asynchronous shutdown. Runtime drives execution independently of subscribers. Do not seize process cwd, environment, signals, global logging, or unrelated processes. Every resource has an owner; slow observation must not become unbounded buffering or lost committed results.

**Proposed minimal permission boundary:** one asynchronous host authorization entry receives the actual action, Agent identity, final arguments, and execution directory, then returns allow or deny. The host may decide automatically or ask its user before returning; the core enforces the decision before dispatch and records it. No rule DSL, role hierarchy, remembered-grant system, or approval workflow framework is required. Errors, cancellation, or unavailable required approval do not authorize execution. Tool authorization is not OS process confinement; allowing shell execution does not prove all its effects stay in the workspace. Exact API and default policy remain to be confirmed.

Do not start with all-plugin composition, hot replacement of the loop, generic event middleware, multiple storage backends, distributed writers, Team management, workflow DSLs, or transparent crash-resumption of arbitrary processes. The WebUI rewrite is abandoned; DSH is a source of contracts, not the architecture to reproduce.

## Rewrite scope and next usable result

The existing release has one user and does not need continued maintenance. **The rewrite has no old-API, old-address, or old-Session-format compatibility requirement.** Do not build adapters, migrations, or dual runtimes just to preserve the old version. Retaining source/data as a reference is not a maintenance commitment, and this decision does not authorize deleting user files.

This checkpoint changes documentation only. Recommended workflow: commit reviewed direction documents, then explicitly create a normal rewrite branch/worktree; main is a reference, not a second development line.

The first usable result is a **public-SDK Rust caller** that opens a Session in a plain non-Git directory, submits input, executes a multi-tool Step, returns a result, cancels/cleans up, and reopens new-format history. Start with `/root`, reusable providers/tools, and generic bounded output; CLI uses the same public interface. **Integrate a real model and real tools early to complete a useful task, not just a greeting.** Validate design through implementation, not a separate pre-implementation proof stage. Focused tests protect concrete contracts; passing mocks alone does not establish usability.

Keep SDK/type/event schemas, commit barriers, storage thresholds, output quotas/parsers, and handoff details open until the next usable result needs them. Distinguish cumulative tokens, request size, disk bytes, and RAM; optimize useful work per model call and precise first admission rather than cache-hit percentage alone. [Evolution](evolution.md) holds optional implementation evidence, not more product prerequisites.
