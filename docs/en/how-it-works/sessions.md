# Sessions

A `Session` is one directory and one ledger. kuku does not keep a separate database record as the source of truth.

## Canonical Mental Model

- A session is one ledger.
- A conversation is one chat thread inside that ledger.
- An agent is a contact card discovered from agent files.
- A conversation address is the continuity key. Reuse the same address to continue the same thread.

## Layout

Sessions live under the workspace-specific area inside `$KUKU_HOME`:

```text
$KUKU_HOME/p/<workspace-path>/sessions/<id>/
|- lock
|- events.jsonl
`- pre-revert-<id>/
```

`events.jsonl` is the durable ledger. Conversations are encoded in events inside that ledger, not as primary `subs/` compatibility directories.

## Event Log

Each line in `events.jsonl` is one persisted fact. Canonical conversation events include:

- `session.created`
- `conversation.opened`
- `conversation.bound`
- `prompt.snapshot`
- `message.user`
- `message.assistant`
- `turn.started`
- `turn.completed`
- `turn.cancelled`
- `turn.interrupted`
- `context.skills`
- `tool.call`
- `permission.requested`
- `permission.allow`
- `permission.deny`
- `tool.result`
- `handoff`
- `conversation.rollback`
- `conversation.rollback.undone`

Older ledgers may still contain compatibility events such as `session.meta`, `turn.end`, or `turn.rollback`.

The full event set is documented in [Events](../reference/events.md).

Readers trust file order. Trailing partial lines are ignored during recovery.

## Conversations Inside One Session

The `main` conversation is the host thread. Agent conversations such as `review` or `review/api` live beside it in the same ledger.

- `main` is reserved for the host conversation.
- `review` and `review/api` are valid conversation addresses.
- Reusing `review` continues that thread.
- Opening `review/api` creates a distinct nested thread whose root contact is `review`.

Conversation-scoped replay, notices, interruption recovery, and rollbacks all work by filtering the same ledger to one address.

## Observability Logs

`$KUKU_HOME/logs/` is the observability tree:

```text
$KUKU_HOME/logs/
|- session/<session-id>.jsonl
|- runtime/<yyyy-mm-dd>.jsonl
`- host/cli|server|webui/<yyyy-mm-dd>.jsonl
```

Logs are for host and runtime visibility. Retention and defaults are configured under [`[logs]`](../reference/config.md#logs).

## Lifecycle

### New session

Starting a run without a session id creates a new session directory and writes `session.created` before the first conversation turn.

### Continuing a session

Starting a run with an existing session id appends a new turn to that same ledger. kuku rebuilds prior context by replaying the ledger and filtering to the active conversation.

If a previous run stopped after `permission.requested` and before `permission.allow`, `permission.deny`, or `tool.result`, restart can recover that unresolved permission state from `events.jsonl`.

### Status

Session status is ledger-wide:

| Status | Meaning |
|---|---|
| `Active` | A live writer lock exists. |
| `Done` | No lock exists and the most recent main-turn terminal event is complete. |
| `Interrupted` | No lock exists and the ledger ended mid-turn or after interruption. |

Conversation status is separate and can be listed per address. See [Manage Sessions](../guides/manage-sessions.md).

## Writer Lock

Only one writer may append to a session at a time. Read operations can happen concurrently.

## Replay and Handoff

Replay is conversation-scoped:

- `main` replay reads historical host-turn facts plus main-thread tool activity.
- agent conversation replay reads only that address plus its tool activity.
- handoff compresses older history and leaves a summary boundary for future replay.

The next request keeps a small number of recent turns and replaces older history with the handoff summary.

## Rollback

Rollback is append-only. kuku records rollback marker events instead of deleting history.

Conversation rollbacks are scoped by address:

| Scope | Effect |
|---|---|
| `conversation_only` | Hides later events from future replay for that conversation. |
| `files_only` | Reverts workspace files to an earlier state without hiding later messages. |
| `both` | Applies both behaviors. |

Main-conversation rollback can hide historical host-turn facts. Agent-conversation rollback hides later events only for that address.

File rollback uses snapshots already captured in `tool.result` data and stores pre-revert backups in `pre-revert-<id>/`.

## Cancellation and Interruption

- Cancellation ends a conversation turn with `turn.cancelled`.
- Crash, resume-before-new-turn, or mid-run stop ends a conversation turn with `turn.interrupted`.
- Runtime notices can surface interrupted turns, pending permissions, open conversations, inbox messages, and loaded skills.

## Session Operations

Hosts can list sessions, list conversations within a session, inspect the full ledger, filter events by conversation, continue a specific thread, or delete the session directory.

See [Agent Loop](agent-loop.md) for turn execution and [Host Apps](../architecture/host-apps.md) for host surfaces.
