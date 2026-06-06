# Agent Loop

Nothing counts as a session fact until it is written to `events.jsonl`.

## Turn flow

```text
turn.start
  -> user.input
  -> model.response
      stop_reason = tool_use ?
        yes -> tool.call -> permission.requested -> permission.allow|permission.deny -> tool.result -> loop
        no  -> turn.end
```

## Per turn

1. kuku appends `turn.start` and `user.input`.
2. It rebuilds the model context from files and persisted events.
3. It streams the model response to the host and appends `model.response` when complete.
4. If the response ends the turn, kuku appends `turn.end`.
5. If the response asks for tools, kuku appends `tool.call`, records pending permission with `permission.requested`, records the permission decision, executes allowed tools, appends `tool.result`, and rebuilds context for the next model call.

## Tool execution

Independent tool calls can run in parallel. kuku preserves the model's original `tool.call` order when it writes results back to the event log.

Subagents use the same loop in child sessions. They do not create a second runtime model.

## Permissions inside the loop

The model can request a tool, but the runtime decides whether that tool may execute. Permission checks are runtime enforcement, not prompt advice.

`permission.requested` is the durable pending state for an unresolved tool authorization request. It is not an allow or deny decision, and it is not read from observability logs.

See [Permissions](permissions.md) for the policy model.

## Handoff and rollback

Two session behaviors change what the next model call sees:

- handoff compresses earlier history into a structured summary when context gets large
- rollback appends marker events that remove prior turns from future context rebuilds, and can also revert files

See [Sessions](sessions.md) for both behaviors.

Runtime streams and observability logs are separate from the session fact log. See [Events](../reference/events.md) and [Sessions](sessions.md#observability-logs).

## Maintainer view

This page describes observable runtime behavior. For crate boundaries and the exact assembly order, see [Architecture Overview](../architecture/overview.md), [Prompt Assembly](../architecture/prompt-assembly.md), and [Module Contracts](../architecture/module-contracts.md).
