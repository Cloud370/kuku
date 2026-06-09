# Module Contracts

This page defines what each SDK module owns, what it may depend on, and what it must not do.

## Core Rule

Each module should have one clear job. Dependency rules keep the ledger model understandable and hard to misuse.

## Ownership Map

| Module | Owns | May depend on | Must not |
|---|---|---|---|
| `query/` | run orchestration, turn execution, slot lifecycle | all other modules | — |
| `conversation/` | conversation addresses, bindings, reduction, conversation-scoped status | `event/`, `context/` | provider logic, host transport |
| `agent/` | agent definitions, catalog, registry, delegated-run preparation | `conversation/`, `tool/`, `query/`, `session/` | unrelated host concerns |
| `context/` | message replay, provenance, rollback planning | `event/`, `conversation/`, `prompt/`, `config/` | provider calls, permission decisions |
| `provider/` | protocol conversion for model APIs | canonical messages, tool schemas, config | session state, event store, permissions |
| `tool/` | definitions, registry, dispatch, built-ins | `event/`, `conversation/`, shared context types | provider protocol, slot scheduling policy |
| `permission/` | runtime gate decisions | `event/`, `conversation/` | tool execution, provider logic, session ownership |
| `session/` | paths, lock, list, delete, session-wide status | `event/`, `conversation/` scan helpers | provider logic, prompt ownership |
| `event/` | event types and `events.jsonl` storage | — | provider, tools, permissions |
| `prompt/` | prompt assets and rendering | — | runtime decisions, session state |
| `config/` | config parsing, validation, defaults, patching | — | provider logic, tool execution |
| `skill/` | skill definitions, loader, registry, loaded-skill recovery | `prompt/`, `event/` | provider logic, host transport |
| `plugin/` | package discovery, manifests, hook execution, registry | `event/`, `config/`, `skill/` | provider logic, permission logic |
| `notice/` | runtime notices such as inbox, interruptions, open conversations, drift | `event/`, `conversation/`, `prompt/`, `skill/`, `agent/` | provider logic |

## Important Boundaries

### `query/`

`query/` is the only orchestration layer that may depend on everything else. It advances the run state machine and turns module outputs into the live agent loop.

### `conversation/`

`conversation/` owns the canonical low-mental-model abstraction: one ledger, many threads. Address parsing, continuity, binding status, and conversation reduction belong here, not in `agent/` or `session/`.

### `agent/`

`agent/` owns contact-card discovery and delegated-run preparation. It does not own the ledger itself.

### `event/`

`event/` is the only module that writes to `events.jsonl`. This is one of the main integrity boundaries in the SDK.

### `context/` and `prompt/`

`prompt/` renders assets. `context/` assembles requests and replays conversation history. Keeping those separate avoids mixing text templates with runtime state decisions.

### `provider/`

Provider adapters own protocol translation only. They should not grow ledger, conversation, or permission behavior.

## Related Pages

- [Prompt Assembly](prompt-assembly.md)
- [Extension Runtime](extension-runtime.md)
- [Code Style](../contributing/code-style.md)
