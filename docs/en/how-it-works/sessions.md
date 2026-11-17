# Sessions

A `Session` owns the persistent history, agents, writer, command output, and active runtime resources for one group of agents. Embed the public SDK directly; no daemon or CLI subprocess is required. See the [SDK reference](../reference/sdk.md) for operations and the calling program.

## Identity and ownership

Agents have stable, flat session-local addresses such as `main`, `reviewer`, or `agent-3`. Retained addresses cannot be reassigned. Creating an agent starts independent context; reusable behavior presets supply initial instructions and defaults without copying another agent's history. Explicit agent configuration affects only that agent.

## Delivery and advancement

Creation, configuration, delivery, and replies accept caller command identities. The same command and content return the original durable receipt; conflicting reuse fails. Delivery, model acceptance, and execution completion are separate facts.

`Activation::Wake` requests advancement; `Activation::Enqueue` only stores input until `advance`. A running agent admits a bounded batch at its next turn boundary. One execution chain advances per agent; different agents can run concurrently. Cancellation keeps an agent stopped until an explicit `advance`.

Execution continues independently of observations. `events(after, limit)` returns bounded committed pages. `observe(after)` combines a durable reconnectable cursor with bounded transient updates. Slow consumers receive a lag notice and retain access to durable results.

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

`cancel` stops new dispatch and awaits owned work cleanup. Unix process groups and Windows Job objects provide process-tree cleanup; deliberate escape is outside the trusted local process boundary. Inherited pipes have a cleanup deadline. Known incomplete cleanup is persisted and makes cancellation or close report failure.

`close().await` awaits cleanup and durable writes before releasing the writer. Pending inputs and recoverable waits remain. Opening does not automatically activate agents. Interrupted side effects require reconciliation and are not automatically executed again. Provider traces remain optional diagnostic logs, separately from session facts.
