# Core Evolution — Implementation Notes

## Purpose

[Core Direction](core-direction.md) owns product and architecture commitments. This page owns staged delivery, implementation evidence, and validation details. Existing [Sessions](../how-it-works/sessions.md), [Prompt Assembly](prompt-assembly.md), and [Module Contracts](module-contracts.md) describe the current release; they are not a requirement to retain coupled runtime ownership.

## Active batch: public-SDK execution slice

Checkpoint 2026-11-17: planning/documentation only. The existing integration branch is `rewrite/core` in `/code/kuku-rewrite`, based on `bacfb7a`; `/code/kuku` on main remains a reference. No rewrite runtime, real-model acceptance, or performance result has been delivered. Updating this plan does not authorize implementation. Keep subsequent status and evidence here, not in parallel handoff documents.

Documentation validation: `git diff --check` and read-only checks of local links/anchors, baseline length, bilingual structure, and technical literals passed. No runtime tests were run.

### Observable result and stages

The first result is a normal Rust caller using only public SDK APIs, plus a thin CLI using those same APIs. In a disposable ordinary directory, an Agent inspects files, changes a small program, runs its tests, and returns an inspectable result. Close and reopen the Session to continue work, and exercise cancellation with asynchronous cleanup. Validate the same core behavior in Git and non-Git directories; no implicit repository setup.

| Stage | Deliverable | Evidence |
|---|---|---|
| 1. Usable single Agent | Public caller and thin CLI; exact model connection, real file/command tools, multi-call Steps, durable acceptance/results, query/observation, cancellation, bounded output, and reopen | One useful real task with its verifier; focused persistence, authorization, output, and cleanup checks |
| 2. Independent Agents and providers | Two independently initialized Agents in one Session using different providers; direct communication and continuation; shared configuration resolution without global mutation | A real delegated task; all three required wire protocols exercised; no parent-transcript fork; same-protocol connection selection; supported reasoning/service settings preserved or unsupported explicit values rejected |
| 3. Durable waiting and orchestration | Persisted question/deferred authorization and matching continuation across reopen; small SDK callers exercising controlled discussion and a timer/external input | No inference while waiting; duplicate input creates no duplicate work; stale replies/callbacks cannot advance replacement work; examples reuse the existing executor |

Stages are delivery boundaries, not separate runtimes. Design identifiers, input semantics, and ownership for these cases from the start, but implement only paths the current stage needs. Stage 2 follows the first useful execution early enough to expose single-model, single-context, and root-only assumptions. Stage 3 examples do not require a production scheduler, Team platform, server, or new UI.

The first stage includes a minimal local CLI; full interactive polish and other hosts remain later work. Complete each stage's required correctness barriers before claiming delivery. A first model experiment may precede completion of all persistence work, but cannot serve as acceptance for missing contracts.

### Implementation ownership and order

1. One initial author establishes the shared SDK boundary and a real vertical path, reusing a provider and a small tool set. Keep Prompt assets and context/provider separation throughout. Do not create empty interfaces for every imagined future capability.
2. Consolidate Session writing and runtime ownership; decouple execution from observation, then integrate bounded output and cleanup. Shared SDK types, command receipts, and the writer/runner interface have one owner.
3. Parallel implementation, when authorized, follows a stable shared interface: at most one storage/recovery line and one Step/provider/tool/output line, with explicit file ownership. Each uses its own worktree from the same integration commit. If the interface is still tightly coupled, keep one author.
4. The integration owner reviews actual diffs and verification evidence before integration. Reuse passing evidence unless new changes or specific failures justify more checks. Update the existing owning documents alongside changed behavior.

The next implementation action, after authorization, is to use the evidence below to assign the first interface and file scope. No development agents were started by this planning update.

### What may be decided during implementation

Rust names/signatures, event framing/schema, numeric queue/output limits, parser choices, and checkpoint layout remain engineering choices under Core Direction. Record concrete limits and overflow behavior before claiming bounded operation. A different identity scope, mandatory Task object, fork default, global model mutation, or weaker durability requires an explicit baseline change.

Live acceptance still needs an available provider/model and existing credential configuration before paid calls. Do not infer SDK credentials from the coding agent's own model, and do not put secrets or private task payloads into documentation. This is an environment prerequisite, not an unresolved product direction.

### Protocol acceptance and fresh-session handoff

The user wants a new Codex session with fresh conversation context to perform the implementation. That session starts in the integration worktree identified above, reads its `AGENTS.md`, Core Direction, and the relevant parts of this page, and follows the existing delivery stages. It must not substitute the main worktree's older baseline or depend on this conversation being available. Implementation begins with the user's explicit instruction to that new session.

The user selected direct DeepSeek `deepseek-flash` as the main low-cost benchmark model and OpenRouter's [`deepseek/deepseek-v4.1-flash-1017`](https://openrouter.ai/deepseek/deepseek-v4.1-flash-1017) for the second gateway. As documented on 2026-11-17, each service exposes the three protocol routes below. These are test connection inputs, not newly frozen SDK configuration names; endpoint availability has not been verified with a paid request here.

| Service | Wire protocol | Base URL | Request suffix | Source |
|---|---|---|---|---|
| DeepSeek | Chat Completions | `https://api.deepseek.com` | `/chat/completions` | [First call](https://api-docs.deepseek.com/) |
| DeepSeek | Responses | `https://api.deepseek.com` | `/responses` | [Responses compatibility](https://api-docs.deepseek.com/guides/responses_api/) |
| DeepSeek | Messages | `https://api.deepseek.com/anthropic` | `/v1/messages` | [Messages compatibility](https://api-docs.deepseek.com/guides/anthropic_api/) |
| OpenRouter | Chat Completions | `https://openrouter.ai/api/v1` | `/chat/completions` | [Quickstart](https://openrouter.ai/docs/quickstart) |
| OpenRouter | Responses | `https://openrouter.ai/api/v1` | `/responses` | [Responses reference](https://openrouter.ai/docs/api/api-reference/responses/create-a-response) |
| OpenRouter | Messages | `https://openrouter.ai/api/v1` | `/messages` | [Messages reference](https://openrouter.ai/docs/api/api-reference/anthropic-messages/create-a-message) |

Start with DeepSeek in stage 1 and exercise the three protocols on both services before declaring stage 2 complete. Each needs streaming output, a real tool-call/result continuation, multi-turn context, and available usage/settings reporting. Keep repeated benchmark runs primarily on DeepSeek; use a small fixed task for OpenRouter compatibility checks rather than rerunning every benchmark on every model/protocol combination. The selected pair covers two access paths within the same model family; it does not establish cross-family model coverage. Additional model families remain optional follow-up validation, not a requirement to buy or select two other models before the initial delivery. Include a delegated task using direct DeepSeek and an OpenRouter-backed Agent with independent contexts.

For protocol-only checks, use the lowest supported enabled reasoning effort. DeepSeek currently documents `low` as that effort in its [thinking guide](https://api-docs.deepseek.com/guides/thinking_mode/); validate the chosen OpenRouter upstream's supported settings and encode them for the selected API rather than assuming all endpoints accept the same fields or values. Benchmark effort is chosen for the case and budget, recorded explicitly, and kept fixed within a comparison. Changes require a separately identified run, not silent escalation after failure. These settings apply to models under test, not to the implementing Codex session's or its development agents' model policies.

For repeatable OpenRouter checks, select one exact upstream endpoint through `provider.order`, set `provider.allow_fallbacks = false` and `provider.require_parameters = true`, and avoid automatic model selection or model fallback. Its [routing documentation](https://openrouter.ai/docs/guides/routing/provider-selection) explains that default routing can otherwise fall back or pass unsupported parameters to a provider that ignores them. Record requested model, gateway, selected route, and reported upstream when available; an unavailable confirmation remains unknown. These are test connection options, not global runtime scheduling rules.

Use focused native-protocol fixtures for rare or nondeterministic errors, batching, and provider-specific replay data that a gateway may normalize. OpenRouter coverage is gateway coverage; it is not evidence that the same requests work against official OpenAI or Anthropic endpoints. Direct official routes remain useful additional checks when credentials are available, without making new accounts a prerequisite for starting the rewrite. Record exactly which paths ran, and mark missing live coverage as unverified.

DeepSeek Responses is stateless and does not support `previous_response_id`; its Messages compatibility layer ignores options including `service_tier` and `cache_control`. A successful HTTP response cannot prove those settings took effect. Keep unsupported explicit choices visible and distinguish these endpoint capabilities from the wire protocol itself.

A local credential template has been prepared outside Git at `/code/.secrets/kuku-acceptance.env`, with directory mode `700` and file mode `600`. The user fills it in an editor. `KUKU_TEST_DEEPSEEK_API_KEY` holds the direct DeepSeek key; `KUKU_TEST_OPENROUTER_API_KEY` holds a separate key issued by OpenRouter. These two variables are sufficient for the selected pair: each service's key is reused across its three protocols, with no extra key per protocol or model. Additional independent routes can use `KUKU_TEST_RESPONSES_BASE_URL`, `KUKU_TEST_RESPONSES_MODEL`, `KUKU_TEST_RESPONSES_API_KEY`, and the matching `KUKU_TEST_MESSAGES_BASE_URL`, `KUKU_TEST_MESSAGES_MODEL`, `KUKU_TEST_MESSAGES_API_KEY` fields; those optional fields may remain empty. These are test-input environment names, not built-in Codex settings or a new SDK API. The existing [config reference](../reference/config.md) explains environment-backed credentials.

The test caller explicitly loads the file without echoing it or enabling shell tracing; do not assume another shell's exports survive into a new session. Keep keys out of prompts, committed files, command arguments, and test reports; report only sanitized connection status, usage, and failures. File permissions do not hide credentials from an Agent running as the owning user. This handoff requires the same filesystem and appropriate access; another host/cloud environment needs its own secret provisioning. Obtain the user's paid-test budget before live calls, then record spend estimates and limits with the acceptance evidence. The template was created empty, and no live API tests were run during this documentation update.

## Existing implementation evidence

These observations are static source review at the planning baseline, not benchmark results or proof that the replacement works.

| Existing path | Observation | Rewrite consequence |
|---|---|---|
| [Query observation](../../../crates/kuku/src/query/run.rs) | `Run::next()` advances execution while returning UI events; `Drop` releases the Session lock | A live runtime owns progress and cleanup independently of observer consumption; async close reports completion/failure |
| [Event store](../../../crates/kuku/src/event/store.rs) | Append rescans historical events and opens/flushes the file per record | One owning writer commits incrementally with explicit durability boundaries; observers do not become writers |
| [Command output](../../../crates/kuku/src/tool/builtin/run_command.rs) | Pipe reading accumulates full output before final rendering limits it | Bound capture and views from the first byte; stream to Session-owned output files |
| [Provider configuration](../../../crates/kuku/src/provider/config.rs) | A builder provider override searches by protocol and takes a matching configured connection | Select the intended named connection independently of its protocol, including multiple gateways using the same protocol |
| [Messages authentication](../../../crates/kuku/src/provider/anthropic.rs) | The current Messages adapter constructs `x-api-key` authentication; OpenRouter documents Bearer authentication for its Messages route | Keep connection authentication configurable independently of the Messages wire format; changing the gateway must not require another Agent loop |
| [Pending authorization recovery](../../../crates/kuku/src/query/start.rs) | Reopening reconstructs pending permission requests and rejects interrupted open tool execution | Preserve useful waiting/recovery behavior; an in-memory authorization future alone is insufficient |
| [SDK exports](../../../crates/kuku/src/lib.rs) | Existing exports and private provider/tool modules reflect the current Query/Run API | Expose deliberate model/tool/host adapters and runtime operations without exporting every internal module or preserving obsolete APIs |

Preserve working file-edit conflict checks, provider protocol behavior, native Skills, and relevant tests. Update module ownership documentation when implementation changes dependencies; do not copy the current `agent`/`query` coupling into a second executor.

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

Pick three to five cases from a mature benchmark rather than building a new evaluation framework. Start with one inexpensive real case, then expand. Keep the original task/environment/verifier and record any adaptation. A small manifest or adapter belongs to development tooling, not the SDK runtime dependency graph. The [Harbor task format](https://docs.harborframework.com/tasks/overview) is one candidate format; no particular case has been selected or run.

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

Run documentation checks for documentation-only changes. For runtime changes, run affected checks plus the required Rust integration checks; feature-gated tests must actually be enabled when they are the acceptance evidence. Keep commands/results with the owning stage, without secrets or private logs. No measurements or runtime acceptance have been produced by this planning update.

## Reference evidence and limits

Read these only for the relevant contract. They do not define kuku's package layout, SDK vocabulary, security model, or default scheduling.

- DSH local source revision `39163dd30f703a79079e2d2446545cff3c453738`: [Agent loop](https://github.com/deepseek-ai/deepseek-harness/blob/39163dd30f703a79079e2d2446545cff3c453738/packages/core/agent-loop/README.md), [Team](https://github.com/deepseek-ai/deepseek-harness/blob/39163dd30f703a79079e2d2446545cff3c453738/packages/team/agent-team/README.md), and [model capabilities](https://github.com/deepseek-ai/deepseek-harness/blob/39163dd30f703a79079e2d2446545cff3c453738/packages/llm/llm/src/types.ts). Borrow prepared requests, lifecycle ownership, and Team reuse of ordinary subagents. Its display paths coexist with underlying Session IDs; kuku does not copy that dual identity or all-plugin assembly.
- Anthropic, [Building always-on agents](https://claude.com/resources/articles/building-always-on-agents), 2026-11-13: event-driven work and durable waits support long-lived service without continuous inference. Its database/worker deployment examples do not require kuku to add a database, daemon, or second executor.
- Superagent prototype revision `8c32a9dc5d72c033816c6a8110b37f40cf94fba4`: [domain](https://github.com/deepseek-ai/dsh-superagent-prototype/blob/8c32a9dc5d72c033816c6a8110b37f40cf94fba4/docs/001-domain.md), [effects](https://github.com/deepseek-ai/dsh-superagent-prototype/blob/8c32a9dc5d72c033816c6a8110b37f40cf94fba4/src/app/effects.ts), and [philosophy](https://github.com/deepseek-ai/dsh-superagent-prototype/blob/8c32a9dc5d72c033816c6a8110b37f40cf94fba4/docs/000-philosophy.md). Borrow work/execution separation, durable side-effect intent, routing to existing owners, and attention-aware reporting. Keep its Ticket/Mandate/Subscription platform and GitHub-specific behavior outside the kernel. The public repository is research code, not a performance or reliability result for kuku.
