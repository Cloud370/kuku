# Core Evolution — Delivery Plan

## Purpose

[Core Direction](core-direction.md) owns product and architecture commitments. This page owns staged delivery, implementation rationale, and acceptance conditions. Existing [Sessions](../how-it-works/sessions.md), [Prompt Assembly](prompt-assembly.md), and [Module Contracts](module-contracts.md) describe the current release; they are not a requirement to retain coupled runtime ownership.

## Delivery plan: public-SDK execution slice

This page defines delivery targets and their acceptance conditions. Track work state and run results through the [development workflow](../contributing/development.md), without adding dated checkpoints or progress tables here. Planning changes do not start implementation.

### Observable result and stages

The first result is a normal Rust caller using only public SDK APIs, plus a thin CLI using those same APIs. In a disposable ordinary directory, an Agent inspects files, changes a small program, runs its tests, and returns an inspectable result. Close and reopen the Session to continue work, and exercise cancellation with asynchronous cleanup. Validate the same core behavior in Git and non-Git directories; no implicit repository setup.

| Stage | Deliverable | Evidence |
|---|---|---|
| 1. Usable single Agent | Public caller and thin CLI; exact model connection, real file/command tools, multi-call Steps, durable acceptance/results, query/observation, cancellation, bounded output, and reopen | One useful real task with its verifier; focused persistence, authorization, output, and cleanup checks |
| 2. Independent Agents and providers | Two independently initialized Agents in one Session using different providers; direct communication and continuation; shared configuration resolution without global mutation | A real delegated task; all three required wire protocols exercised; no parent-transcript fork; same-protocol connection selection; supported reasoning/service settings preserved or unsupported explicit values rejected |
| 3. Durable waiting and orchestration | Persisted question/deferred authorization and matching continuation across reopen; small SDK callers exercising controlled discussion and a timer/external input | No inference while waiting; duplicate input creates no duplicate work; stale replies/callbacks cannot advance replacement work; examples reuse the existing executor |

Stages are delivery boundaries, not separate runtimes. Design identifiers, input semantics, and ownership for these cases from the start, but implement only paths the current stage needs. Stage 2 follows the first useful execution early enough to expose single-model, single-context, and root-only assumptions. Stage 3 examples do not require a production scheduler, Team platform, server, or new UI.

The first stage includes a minimal local CLI; full interactive polish and other hosts remain later work. Complete each stage's required correctness barriers before claiming delivery. A first model experiment may precede completion of all persistence work, but cannot serve as acceptance for missing contracts.

### Implementation ownership

The Session SDK owns execution and durable state. Provider adapters own protocol conversion. The acceptance entrypoint checks the public SDK and host boundaries. For the next host changes, see the ownership map and client plan. [Module contracts](module-contracts.md) · [Interactive clients](interactive-clients.md)

### What may be decided during implementation

Rust names/signatures, event framing/schema, numeric queue/output limits, parser choices, and checkpoint layout remain engineering choices under Core Direction. Record concrete limits and overflow behavior before claiming bounded operation. A different identity scope, mandatory Task object, fork default, global model mutation, or weaker durability requires an explicit baseline change.

### Development workflow delivery

The [workflow deliverables](../contributing/development.md#workflow-deliverables-for-the-rewrite) are part of the same rewrite. Stage 1 includes fresh-session handoff and a repeatable public-SDK/CLI acceptance path; stages 2 and 3 extend that path for protocols, independent Agents, and recovery. Align task reporting, test entrypoints, PR guidance, and CI with the delivered behavior. Do not defer the workflow until the runtime is complete or create a separate automation platform.

Use the [live protocol acceptance](../contributing/testing.md#live-protocol-acceptance) for selected models, connection inputs, credentials, and paid-test limits. Run records belong to verification artifacts, not this design plan.

## Implementation evidence

The public caller demonstrates opening and completing a Session. Runtime tests cover concurrent Agents, deferred authorization and cancellation. Context tests verify exact model input across reopen. An HTTP integration test exercises execution after an observer disconnects. [SDK example](../../../crates/kuku/examples/session_task.rs) · [Session runtime tests](../../../crates/kuku/tests/session_runtime.rs) · [Context tests](../../../crates/kuku/tests/context_runtime.rs) · [Server integration](../../../crates/kuku-server/tests/session_execution.rs)

## Runtime and Harness boundary

The default Harness and custom callers both use SDK operations for creation, input, configuration, advancement/control, query, and observation. They may select context material and activation policy; they cannot bypass durable admission, tool authorization, or cleanup. Ordinary internal responsibilities may remain cohesive modules in one crate. Introduce an adapter where an actual external capability varies, not a package and registry for every internal call.

| Future use | What its Harness adds | What the runtime supplies |
|---|---|---|
| Independent delegation | Task brief, selected material, recipient choice, result collection | Fresh Agent context, stable address, execution and message facts |
| Teams / controlled discussion | Membership, optional task board, speaker/termination rules | The same Agents and direct delivery; enqueue without activation when needed |
| Always-on assistant | Durable timers/intake, cross-Session routing, Memory selection, notification policy | Reopenable state, correlated input, waiting, bounded execution, queryable results |
| Work-oriented orchestration | External work identity, assignments, review/completion rules | References linking inputs and results without treating Agent idle as work success |

A long-lived assistant name may route to different Sessions over time. A delayed answer or approval must still target the original Session, Agent, and pending request; it must not follow the name to whichever Session is newest. Business records can live outside the Session log; SDK idempotency and observation let the host reconcile across that boundary without distributed transactions.

## Input, advancement, and observation

Define idempotency scope and command identity before integrating creation and submission. Within the documented retention/scope, an identical retry returns the original receipt; the same key with a different payload fails. Preserve sender attribution from the trusted calling boundary, not from text inside a model-generated message.

Record input acceptance separately from admission into a prepared Step and from execution settlement. The default send operation may combine delivery with an activation request, while controlled Harnesses stage input without running a model. Runtime lookup, batching, and delivery receipts are deterministic operations. An incoming context item is neither an implicit scope expansion nor an approval.

Use one advancing chain per Agent and short Session commit serialization. Model calls, tool I/O, and slow subscribers must not hold the writer across awaits. Bound pending work and define admission rejection/backpressure. A durable query/observation cursor permits reconnection after a lost callback; transient stream chunks need not all be durable, but committed facts must remain discoverable.

## Model resolution, context, and handoff

Use a single configuration resolver. A chosen Preset supplies behavior and initial defaults; a chosen Tier supplies route/settings defaults; explicit Agent choices take precedence. Resolve against the exact configured connection/model and bind the result before request dispatch. Later edits to an Agent do not change others or silently rewrite their defaults. Configuration changes apply at explicit safe boundaries.

Adapter capability metadata and validation serve SDK callers, user interfaces, and model-facing Agent tools. Reasoning and service choices remain separate; provider-specific values must be declared, validated, and recorded rather than silently ignored or forced into a universal ordering. Distinguish requested settings, resolved settings sent, and optional provider-reported settings. Explicitly incompatible choices fail; never claim an unreported setting was confirmed.

Independent initialization supplies only the material the caller selected. Reusing an Agent can preserve its accumulated project knowledge; this does not justify making transcript forks the default. On model/connection changes, do not transplant incompatible inherited options unnoticed. Record the resolved choice and its source.

Retain Prompt assets and the prepared provider-visible content needed for faithful reconstruction. Store admitted content and tool-result views once, with references and request boundaries rather than a full transcript copy on each Step. Historical replay does not re-read mutable workspace files or re-render today's Prompt templates. A context handoff records its predecessor and selected material and commits the successor before switching. If handoff fails, preserve the previous explainable state.

Keep each submitted prefix stable while its segment is active. Bound and reference large results on first admission. Under measured byte or token pressure, choose a cut before a complete tool batch or user input. A bounded summary request checkpoints earlier work. Preserve the current user input, recent complete batches and their provider-native replay. Use the unchanged cached prefix for the summary when it fits. Validate the replacement request before committing a successor. The original journal and retained bytes stay available under the existing retention and authorization rules. A failed summary leaves the submitted segment authoritative.

Keep a finished user turn together or record its outcome in the checkpoint, including replies already delivered. Preserve the original input of work that is still open. Use one short, optional correction section in the checkpoint. Each entry ties an observed failure or explicit correction to evidence and a concrete change for the remaining work. Preserve the smallest verified lookup for decisive evidence, such as a path and symbol or an output ID and byte range. Omit weak inferences. The resumed Agent can check uncertain conclusions against the retained evidence. Rewinding across the summarized work invalidates its checkpoint and restores the surviving history.

## Persistence, effects, and durable waiting

Start with one authoritative append-only log and one writer per Session, with cross-process exclusion and stale-owner fencing. Bounded record sizes and external output permit streaming recovery. Small disposable indexes can serve Agent lookup, command deduplication, or event positions; choose additional persisted indexes/checkpoints against actual recovery and lookup measurements.

Define the commit unit and acknowledgement barriers explicitly. Referenced output must exist durably before its confirmed reference; an action's dispatch intent precedes the effect, and settlement precedes confirmation. Recover only identifiable incomplete tails; reject corruption of committed history. A required write failure stops dependent execution.

A requested action, an action known to have been dispatched, and a confirmed result are different facts. If a side effect may already have happened, report an unknown outcome. A provider/tool adapter may reconcile or use a supported external idempotency mechanism; generic retry must not repeat arbitrary effects. Reject stale execution callbacks after cancellation/replacement using the bound execution identity.

Persist resumable waits only at defined boundaries, such as a question or authorization before dispatch. Retain the request identity, exact target action and arguments/tool binding where applicable, and the reply correlation. An async callback may handle a short wait, but is not the durable record. Resume verifies that the request is still pending and applicable; duplicate or stale answers cannot dispatch twice. Do not recover a suspended Rust future or an arbitrary half-finished OS process.

Closing live Session resources stops/settles owned execution and releases the writer after required writes. It does not declare business tasks successful or erase pending facts. Reopening rebuilds those facts for authorized Harness scheduling; whether unattended work resumes is an explicit host policy.

## Tools, workspace, and resource budgets

Foreground and background work use one execution and output lifecycle. A ToolCall returns one settlement: a final result or a Job handle. Job completion is a later fact. A parent Turn ending does not implicitly end child Agents; cancellation scope follows explicit ownership and control, not address spelling.

Stream stdout/stderr through bounded buffers, parser state, diagnostics, and views. Begin with a generic preview and only useful explicit build/test parsers. Mark filtering or incomplete capture and expose output references/ranges. Define quota and disk-full behavior before promising complete capture; stop or drain/discard explicitly so full pipes cannot deadlock. Keep references for the promised retention period.

Use ordinary directories with explicit cwd and environment for each execution. Shared workspace defaults do not provide transaction isolation; preserve stale-edit detection and order known conflicting tools. Worktrees and Git remain user/Harness choices. SDK code must not take over global cwd, environment, signal handlers, logging, or unrelated processes.

Treat model requests, tool processes, active context, pending input, output storage, and subscribers as distinct resources. Waiting for human input releases model capacity. Share immutable Prompt/tool assets and suitable clients, avoid full-history copies per Agent, and permit inactive state to be reconstructed on demand. A fair Harness may prioritize human follow-ups, but must respect explicit hard resource limits.

The local default entry selects the trusted-local policy described in Core Direction. Embedded hosts can replace it. Do not reintroduce an unresolved deny-by-default product gate or make a policy DSL prerequisite; errors, missing required decisions, and cancellation still never authorize an effect.

## Validation and measurements

Pick three to five cases from a mature benchmark rather than building a new evaluation framework. Start with one inexpensive real case, then expand. Keep the original task/environment/verifier and record any adaptation. A small manifest or adapter belongs to development tooling, not the SDK runtime dependency graph. The [Harbor task format](https://docs.harborframework.com/tasks/overview) is one candidate format.

Record provider/connection, model, reasoning, service, budgets, and environment so comparisons are meaningful. Inspect output artifacts, original verifier results, process exit status, committed execution facts, and admitted model input. Do not assert model wording or incidental tool order. Separate setup/infra failure, runtime failure, and an agent failing the task. A tiny case set is diagnostic, not a leaderboard claim.

Choose focused tests as each boundary is implemented:

- Duplicate creation/input and lost receipts; wrong-target, duplicate, or stale replies; no duplicate advancing Agent driver.
- Competing writers, incomplete tails versus committed corruption, disk/write failures around dispatch/settlement, unknown effects, and stale callbacks.
- Multiple same-protocol connections, independent provider choices, unsupported reasoning/service settings, and no mutation of other Agents.
- Independent Agent contexts, controlled activation, direct authorized messages, and no forced parent-history fork.
- Reopened provider-visible prefixes, changed templates/configuration, failed handoff, and exact first-admitted output views.
- Multi-call pairing, conflicting edits, authorization denial/error/defer/cancel, large output, cleanup races, and slow/disconnected observers.
- A persisted wait across reopen and a minimal external timer or controlled-discussion caller using the same executor.

Use the real storage, authorization, and execution path where possible; replace only the dependency needed for deterministic fault injection. A real model may not emit multiple calls on demand, so pair a real task with a focused batch test when necessary. Neither mocks alone nor one successful live run establishes all failure guarantees.

Measure idle and active memory/CPU, cold open/recovery, append and context preparation latency, large-output behavior, and concurrent Agents. Separate SDK process use from tools and benchmark containers. Record uncached input, cache reads/writes, output, actual/estimated cost, and user-visible latency; distinguish first exploration from continued work. Do not substitute a cache-hit percentage or cumulative token count for these costs, and never promise a fixed provider cache hit rate.

Follow the [testing workflow](../contributing/testing.md) for required checks, explicit feature selection, and evidence reporting. A direct API probe does not establish SDK acceptance.

## Reference evidence and limits

Read these only for the relevant contract. They do not define kuku's package layout, SDK vocabulary, security model, or default scheduling.

- DSH local source revision `39163dd30f703a79079e2d2446545cff3c453738`: [Agent loop](https://github.com/deepseek-ai/deepseek-harness/blob/39163dd30f703a79079e2d2446545cff3c453738/packages/core/agent-loop/README.md), [Team](https://github.com/deepseek-ai/deepseek-harness/blob/39163dd30f703a79079e2d2446545cff3c453738/packages/team/agent-team/README.md), and [model capabilities](https://github.com/deepseek-ai/deepseek-harness/blob/39163dd30f703a79079e2d2446545cff3c453738/packages/llm/llm/src/types.ts). Borrow prepared requests, lifecycle ownership, and Team reuse of ordinary subagents. Its display paths coexist with underlying Session IDs; kuku does not copy that dual identity or all-plugin assembly.
- Anthropic, [Building always-on agents](https://claude.com/resources/articles/building-always-on-agents): event-driven work and durable waits support long-lived service without continuous inference. Its database/worker deployment examples do not require kuku to add a database, daemon, or second executor.
- Superagent prototype revision `8c32a9dc5d72c033816c6a8110b37f40cf94fba4`: [domain](https://github.com/deepseek-ai/dsh-superagent-prototype/blob/8c32a9dc5d72c033816c6a8110b37f40cf94fba4/docs/001-domain.md), [effects](https://github.com/deepseek-ai/dsh-superagent-prototype/blob/8c32a9dc5d72c033816c6a8110b37f40cf94fba4/src/app/effects.ts), and [philosophy](https://github.com/deepseek-ai/dsh-superagent-prototype/blob/8c32a9dc5d72c033816c6a8110b37f40cf94fba4/docs/000-philosophy.md). Borrow work/execution separation, durable side-effect intent, routing to existing owners, and attention-aware reporting. Keep its Ticket/Mandate/Subscription platform and GitHub-specific behavior outside the kernel. The public repository is research code, not a performance or reliability result for kuku.
