# Sessions

A `Session` owns the persistent history, agents, writer, command output, and active runtime resources for one group of agents. Embed the public SDK directly; no daemon or CLI subprocess is required. See the [SDK reference](../reference/sdk.md) for operations and the calling program.

## Identity and ownership

Agents have stable, flat session-local addresses such as `main`, `reviewer`, or `agent-3`. Retained addresses cannot be reassigned. Creating an agent starts independent context; reusable behavior presets supply initial instructions and defaults without copying another agent's history. Explicit agent configuration affects only that agent.

## Delivery and advancement

Creation, configuration, delivery, replies and targeted execution controls accept caller command identities. The same command and content return the original durable receipt. Conflicting reuse fails. Delivery, model acceptance, and execution completion are separate facts.

`Activation::Wake` requests advancement. `Activation::Enqueue` stores input for explicit host scheduling. A running Agent admits eligible inputs in receipt order at a safe boundary. `InputTiming::NextStep` allows the current Turn's next model request, after the in-flight response and dispatched tools settle. If the Turn settles first, the input remains eligible for the next Turn. `InputTiming::NextTurn` waits for the next Turn even when a later `NextStep` input can be admitted sooner. A technical retry reuses the prepared request.

One execution chain advances per Agent, and different Agents can run concurrently. A targeted cancellation uses the observed execution identity. Queued inputs remain visible after cancellation. Explicit advancement or a fresh waking host input resumes stopped work. A queued input can be withdrawn by its receipt identity. A withdrawal racing with acceptance returns the actual state.

Execution continues independently of observations. `agent_view` returns an Agent snapshot, queued inputs and the corresponding committed cursor. `events(after, limit)` returns bounded committed pages. `observe(after)` combines a durable reconnectable cursor with bounded transient updates identified by Agent and execution. Slow consumers receive a lag notice and retain access to durable results.

## Storage and context

The [session layout](../reference/file-layout.md#session-layout) contains one authoritative `events.jsonl` journal and retained outputs. One shared writer holds an operating-system lock and incrementally appends synchronized records. Opening repairs an incomplete tail and rejects damaged committed history.

Agent creation/configuration, delivery/acceptance, prepared steps, authorization, dispatch, results, waits, replies, and settlements are separate facts. A completed turn or model claim does not prove business success; inspect actual files, command results, and the task judge.

Provider-visible system text, prelude, tools, and admitted canonical messages are frozen in a context segment. Messages are admitted once per segment; each prepared step records the segment and message boundary, selected connection/settings, correlation, and request hash. It does not append another full wire request. Reopening keeps the committed prefix. Configuration, tool, protocol, or explicit handoff changes establish a committed successor segment.

Behavioral Prompt assets remain external or embedded versioned resources, separately from context selection and provider conversion. Protocol-native continuation data remains attached to its assistant response and cannot silently cross an incompatible protocol.

## Waiting and replies

`ToolAuthorizer` receives the trusted source, agent, final arguments, actual tool definition, and working directory. The default local policy is `TrustedLocal`. Denial or an authorization error prevents dispatch. `Defer` persists an exact pending action and releases active execution context. A matching `reply_permission` checks the prepared action and current tool definition before resuming once.

`ask_user` persists a question and returns a waiting handle once. The agent yields after its current tool batch. `reply_question` delivers correlated input only to the matching pending request. Reopening reconstructs waits; unrelated, conflicting duplicate, or stale replies do not authorize another action. Waiting does not poll a model or hold a model permit.

## Output and cleanup

Command stdout/stderr stream from the first byte through bounded frames to a session-owned file. The first model view is a bounded preview or recognized build/test summary with a retained reference and explicit filtering/incompleteness flags. `read_output` supports bounded ranges, literal search, and incremental offsets. Exceeding quota keeps draining pipes and marks capture incomplete.

An Agent can give the capture a [named value](../reference/tools.md#named-session-values). Ordinary captures can age out. Naming an output or attaching it to a delivered input retains the referenced bytes for reuse on reopen. A replaced name leaves delivered references intact. Write long-lived deliverables to the workspace as ordinary files.

`cancel` stops new dispatch and awaits owned work cleanup. Unix process groups and Windows Job objects provide process-tree cleanup; deliberate escape is outside the trusted local process boundary. Inherited pipes have a cleanup deadline. Known incomplete cleanup is persisted and makes cancellation or close report failure.

`close().await` awaits cleanup and durable writes before releasing the writer. Pending inputs and recoverable waits remain. Opening does not automatically activate agents. Interrupted side effects require reconciliation and are not automatically executed again. Provider traces remain optional diagnostic logs, separately from session facts.
