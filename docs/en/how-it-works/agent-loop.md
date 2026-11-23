# Agent loop

A **Turn** begins when an Agent admits work from its inbox. Inside a Turn, each **Step** is one logical model request plus whatever tool calls that response makes. A Turn can span many Steps.

## From input to result

1. **Commit & admit** — The Session commits the submitted input and returns a receipt. The runtime admits eligible inputs when a Turn starts (or at a safe step boundary).
2. **Prepare & request** — The SDK prepares the Agent's context, journals the Step metadata, and sends the model request. Clients can stream partial output while the response is in flight.
3. **Validate & run tools** — If the response asks for tools, the runtime validates the entire batch first, binds each call to its resolved arguments and tool definition, checks authorization, logs dispatch, and commits results. Independent calls can run concurrently; conflicting workspace operations are serialized.
4. **Loop or settle** — The next Step receives the admitted tool-result views. A response with no tool calls ends the Turn, and the Turn succeeds only when its normalized terminal state is successful.

If a Session-owned Agent request hits the model's output token limit, the Turn is interrupted and incomplete tool-call batches are never dispatched. Direct `Query` runs and child-Agent queries can retry an output-limit response once using the same request settings plus a runtime recovery notice; if that retry still hits the limit or cannot be sent, execution interrupts. The incomplete response stays in the journal for inspection but is never replayed into model context.

Read an Agent's settled output with `kuku show SESSION_ID --agent AGENT`. A finished Turn is not proof of task correctness — verify changed files, command output, or tests directly. [Events](../reference/events.md) · [Session SDK](../reference/sdk.md)

## Waiting, interruption, and handoff

- **Waiting**: An `ask_user` question or deferred tool approval yields execution until the host submits the matching reply (even across a Session reopen).
- **Cancellation**: Canceling an Agent stops new dispatch and waits for its owned work to settle.
- **Unknown post-dispatch outcomes**: If a tool is interrupted after dispatch with an unknown external effect, reconcile what actually happened before trying again.
- **Handoff**: When context compaction summarizes older Turns and keeps recent ones, the SDK commits a successor context segment while the prior segment stays explainable in the journal.

[Sessions](sessions.md) · [Prompt assembly](../architecture/prompt-assembly.md)
