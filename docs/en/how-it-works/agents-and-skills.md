# Agents and Skills

Agents and Skills extend what the model can do, but they do it in different ways.

## The difference

| | Skill | Agent |
|---|---|---|
| Execution model | Injects instructions into the current session | Spawns a child session |
| Tools | Uses the current session's tool set | Uses a constrained child tool set |
| Lifetime | Shares the parent session | Ends when the child session ends or reaches its turn limit |
| Best for | Workflow guidance or bundled knowledge | Independent delegated work |

A Skill adds instructions. An Agent adds another executor.

## Skills

Skills are markdown-based capabilities discovered from skill directories. The model loads a Skill when it needs a workflow, reference material, or packaged behavior inside the current session.

Skills do not create separate session state. They change the current session by adding instructions and optional resources.

## Agents

Agents are subagent definitions discovered from user or project directories. When the model uses an Agent, kuku creates a child session and runs the same agent loop there.

That child session has:

- its own event log
- permission requests routed back through the parent run
- a limited depth budget
- its own turn limit

Nested Agent delegation is limited to `parent -> child -> grandchild`. If a child session tries to spawn another Agent beyond that depth, the runtime blocks the call with `blocked: maximum subagent depth (2) reached`.

## Permissions and inheritance

Subagent permission requests are surfaced back through the parent run for a decision. Hard guards still apply.

Skills do not bypass permissions either. They can influence behavior, but the runtime still decides which tool calls are allowed.

See [Permissions](permissions.md) for the enforcement model.

## Where each fact belongs

- This page explains the runtime relationship.
- Usage flows belong in `guides/`.
- File formats belong in `reference/`.
- Package and loader internals belong in [Extension Runtime](../architecture/extension-runtime.md).
