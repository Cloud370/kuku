# Agent loop

A Turn starts when an Agent accepts work from its inbox. A Step is one logical model request and the tools its response asks to use. A Turn can have several Steps.

## From input to result

1. The Session commits the submitted input and returns a receipt. The running SDK admits eligible input at the start of a Turn.
2. The SDK prepares the Agent's context and records the Step before sending the request. Clients can observe partial output while the response is in flight.
3. A tool response starts a batch. The runtime validates the complete batch before binding each call to its arguments and definition, checking authorization, recording dispatch, and committing results. Independent work can run concurrently, and conflicting workspace operations are ordered.
4. The next Step receives the admitted tool-result views. The system protocol treats a response without tool calls as the final answer, but the Turn completes successfully only when its resolved terminal state is successful. An output-limit stop in the Session-owned Agent request interrupts the Turn, and incomplete tool-call batches are never dispatched.

Direct `Query` executions and child-Agent queries can retry an output-limit response once with the original request settings plus a runtime recovery notice. If that retry also reaches the limit or cannot be sent, execution is interrupted. The incomplete response remains in the journal but is not replayed as model context.

Read the Agent's settled result with `kuku show SESSION_ID --agent AGENT`. To verify that a task succeeded, inspect the changed files, command results or task-specific tests. [Events](../reference/events.md) · [Session SDK](../reference/sdk.md)

## Waiting, interruption and handoff

An `ask_user` question or deferred tool authorization can yield execution until the host submits the matching reply. Canceling an Agent stops new dispatch and waits for its owned work to settle. Reopening restores confirmed history and pending requests. A tool with an unknown post-dispatch outcome requires reconciliation before any further attempt.

When a handoff selects a summary and recent Turns, the SDK prepares a successor context segment. The previous segment remains explainable from its journal. [Sessions](sessions.md) · [Prompt assembly](../architecture/prompt-assembly.md)
