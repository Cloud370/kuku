# Agent loop

A Turn starts when an Agent accepts work from its inbox. A Step is one logical model request and the tools its response asks to use. A Turn can have several Steps.

## From input to result

1. The Session commits the submitted input and returns a receipt. The running SDK admits eligible input at the start of a Turn.
2. The SDK prepares the Agent's context and records the Step before sending the request. Clients can observe partial output while the response is in flight.
3. A tool response starts a batch. The runtime binds each call to its arguments and definition, checks authorization, records dispatch and commits the result. Independent work can run concurrently, and conflicting workspace operations are ordered.
4. The next Step receives the admitted tool-result views. A final response with no tool calls settles the Turn with a durable result. A response truncated by the model's output limit interrupts a public Session Turn. Tool calls in that incomplete response are never dispatched.

Read the Agent's settled result with `kuku show SESSION_ID --agent AGENT`. To verify that a task succeeded, inspect the changed files, command results or task-specific tests. [Events](../reference/events.md) · [Session SDK](../reference/sdk.md)

## Waiting, interruption and handoff

An `ask_user` question or deferred tool authorization can yield execution until the host submits the matching reply. Canceling an Agent stops new dispatch and waits for its owned work to settle. Reopening restores confirmed history and pending requests. A tool with an unknown post-dispatch outcome requires reconciliation before any further attempt.

When a handoff selects a summary and recent Turns, the SDK prepares a successor context segment. The previous segment remains explainable from its journal. [Sessions](sessions.md) · [Prompt assembly](../architecture/prompt-assembly.md)
