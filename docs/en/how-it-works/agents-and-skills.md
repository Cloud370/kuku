# Agents and Skills

Agents and Skills both extend the runtime, but they operate at different layers.

## Canonical Mental Model

- A skill is instructions loaded into the current conversation.
- An agent is a contact card discovered from agent files.
- Calling an agent opens or continues a conversation address inside the same session ledger.
- The address is the continuity key.

## The Difference

| | Skill | Agent |
|---|---|---|
| Execution model | Injects instructions into the current conversation | Opens or continues a delegated conversation |
| State model | Shares the active conversation | Gets its own conversation thread in the same session ledger |
| Tools | Uses the current tool surface | Uses the bound agent tool surface |
| Continuity | Persists on the current conversation address | Persists when you reuse the same agent conversation address |
| Best for | Workflow guidance or packaged knowledge | Isolated delegated work |

## Skills

Skills are markdown-based capabilities discovered from skill directories. The model loads a skill when it needs workflow guidance, reference material, or packaged behavior inside the current conversation.

Skills do not create a second conversation. They change the current conversation by adding instructions and optional resources.

The default skill tool surface is `list_skills`, `search_skills`, and `use_skill`.

`context.skills` records the skill registry snapshot and bootstrap-loaded skill names for the conversation turn. Runtime notices can also surface currently loaded skills.

## Agents

Agents are contact cards discovered from user or project directories. The `agent` tool delegates work by sending a message to a conversation address that is bound to one agent identity.

That delegated conversation has:

- its own replay stream inside the shared ledger
- its own terminal events (`turn.completed`, `turn.cancelled`, `turn.interrupted`)
- permission requests routed back through the parent run
- a depth budget for further delegation
- its own continuity when the same address is reused

The `main` conversation is reserved for the host thread and cannot be targeted by the `agent` tool.

Nested delegation is limited to `parent -> child -> grandchild`. If a delegated conversation tries to spawn another agent beyond that depth, the runtime blocks the call with `blocked: maximum agent delegation depth (2) reached`.

## Address and Binding Rules

Conversation addresses are lowercase slash-separated names such as:

- `review`
- `review/api`
- `explore/auth-flow`

Rules:

- `main` is reserved
- `main/...` is invalid
- the root segment must name a known agent contact card
- reusing an existing address means continue that conversation
- a new address gets a fresh `conversation.opened`
- the first bind for an address writes `conversation.bound`

If an existing address is reused, the bound identity must still match. If the agent definition now hashes to a different binding identity, the runtime rejects the call.

## Permissions and Notices

Delegated permission requests are surfaced back through the parent run for a decision. Skills do not bypass permissions either.

Runtime notices can surface:

- open delegated conversations
- inbox messages for the current conversation
- loaded skills
- pending permissions
- interrupted turns
- context drift

See [Permissions](permissions.md) for the enforcement model.

## Where Each Fact Belongs

- This page explains the runtime relationship.
- Usage flows belong in `guides/`.
- File formats belong in `reference/`.
- Package and loader internals belong in [Extension Runtime](../architecture/extension-runtime.md).
