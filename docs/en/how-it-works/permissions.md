# Permissions

Permissions are runtime enforcement.

Project instructions can suggest behavior, but they do not grant or deny tool access by themselves.

## Decision sources

The runtime evaluates permissions in a fixed order:

1. hard guard
2. project policy deny
3. session grants
4. project policy allow
5. default behavior

The hard guard always wins.

## What the model can and cannot do

- The model may request a tool call.
- The runtime decides whether that call is allowed.
- A denied call still becomes an event in the session history.

This keeps the permission system auditable and separate from prompt wording.

## Common modes

Hosts can expose permissions in different ways, but the runtime supports the same underlying choices:

- one-time approval
- session approval
- project approval
- deny

Subagent permission requests are surfaced back through the parent run. Hard guards still apply there as well.

## Policy files and runtime facts

Project permission state is file-backed and evaluated by the runtime. It is not ordinary conversation context.

## Failure shape

If a tool call is denied, kuku records the permission decision and writes a blocked tool result. If a call is allowed, execution continues and the result is written normally.

## Mental model

Permissions are part of the execution boundary between model intent and actual side effects. That boundary is one of the main reasons kuku keeps the runtime separate from host UI and prompt instructions.

For the maintainer-facing boundary, see [Module Contracts](../architecture/module-contracts.md).
