# Docs

Canonical concept names are in [glossary.md](glossary.md).

**Convention**: Features designed but not yet implemented are marked `(planned)`. After implementing, `grep -rn "(planned)" docs/` to find and update every reference.

## Core

How kuku works.

| File | What it covers |
|------|----------------|
| [direction.md](core/direction.md) | Why kuku exists, design philosophy |
| [agent-loop.md](core/agent-loop.md) | Turns, events, tool dispatch, stop conditions |
| [session.md](core/session.md) | Session lifecycle, lock, `$KUKU_HOME` layout |
| [events.md](core/events.md) | Event types, `events.jsonl`, replay, response groups |
| [tools.md](core/tools.md) | Tool model: definition, registry, dispatch, result envelope |
| [memory.md](core/memory.md) | `memory.md` files, remember/forget, context drift |
| [architecture.md](core/architecture.md) | Module dependency map, directory structure, instructions loading |
| [prompt.md](prompt.md) | Prompt layering, assembly order, cache strategy |

## Contributing

Rules for contributors.

| File | What it covers |
|------|----------------|
| [code-style.md](contributing/code-style.md) | Naming, visibility, imports, tests, commits |
| [modules.md](contributing/modules.md) | Module boundaries, dependency rules, provider/tool/context contracts |
