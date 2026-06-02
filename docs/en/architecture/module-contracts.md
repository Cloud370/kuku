# Module Contracts

This page defines what each SDK module owns, what it may depend on, and what it must not do.

## Core rule

Each module should have one clear job. Dependency rules exist to keep the file-native runtime understandable and hard to misuse.

## Ownership map

| Module | Owns | May depend on | Must not |
|--------|------|---------------|----------|
| `query/` | agent loop, `Run`, orchestration | all other modules | — |
| `context/` | message rebuild, provenance, rollback planning | `event/`, `prompt/`, `config/`, shared tool helpers | provider calls, permission decisions |
| `provider/` | protocol conversion for model APIs | canonical messages, tool schemas, config | session state, event store, permissions |
| `tool/` | definitions, registry, dispatch, built-ins | `event/`, shared context types | provider protocol, slot scheduling policy |
| `permission/` | runtime gate decisions | `event/` | tool execution, provider logic, session ownership |
| `session/` | paths, lock, list, delete, status | `event/` scan helpers | provider logic, model state |
| `event/` | event types and `events.jsonl` storage | — | provider, tools, permissions |
| `prompt/` | prompt assets and rendering | — | runtime decisions, session state |
| `config/` | config parsing, validation, defaults, patching | — | provider logic, tool execution |
| `skill/` | skill definitions, loader, registry | `prompt/` | runtime decisions, session state |
| `plugin/` | package discovery, manifests, hook execution, registry | `event/`, `config/`, `skill/` | provider logic, permission logic |
| `subagent/` | agent definitions, registry, child-session spawn | `tool/`, `query/`, `session/` | unrelated host concerns |
| `notice/` | drift detection and notice rendering | `event/`, `prompt/` | provider logic |

## Important boundaries

### `query/`

`query/` is the only orchestration layer that may depend on everything else. It advances the run state machine and turns module outputs into the live agent loop.

### `event/`

`event/` is the only module that writes to `events.jsonl`. This is one of the main integrity boundaries in the SDK.

### `context/` and `prompt/`

`prompt/` renders assets. `context/` assembles requests. Keeping those separate avoids mixing text templates with runtime state decisions.

### `provider/`

Provider adapters own protocol translation only. They should not grow session or permission behavior.

## Related pages

- [Prompt Assembly](prompt-assembly.md)
- [Extension Runtime](extension-runtime.md)
- [Code Style](../contributing/code-style.md)
