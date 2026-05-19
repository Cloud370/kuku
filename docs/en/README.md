# Docs

Canonical concept names are in [glossary.md](glossary.md).

This is the canonical public docs index. The project homepage is [README.md](../README.md).

**Status**: `implemented` = implemented today; `partial` = implemented behavior plus planned pieces; `planned` = not implemented yet; `extension design` = intentionally outside core runtime.

**Convention**: Features designed but not yet implemented are marked `(planned)`. After implementing, `grep -rn "(planned)" docs/` to find and update every reference.

## Core

How kuku works.

SDK owns runtime facts and semantics. Host apps own input, output, layout, and interaction.

| File | What it covers | Status |
|------|----------------|--------|
| [direction.md](core/direction.md) | Why kuku exists, design philosophy | implemented |
| [agent-loop.md](core/agent-loop.md) | Turns, events, tool dispatch, stop conditions | partial |
| [session.md](core/session.md) | Session lifecycle, lock, `$KUKU_HOME` layout | partial |
| [events.md](core/events.md) | Event types, `events.jsonl`, replay, response groups | implemented |
| [tools.md](core/tools.md) | Tool model: definition, registry, dispatch, result envelope | partial |
| [memory.md](core/memory.md) | `memory.md` files, remember/forget, context drift | implemented |
| [architecture.md](core/architecture.md) | Module dependency map, directory structure, instructions loading | implemented |
| [prompt.md](prompt.md) | Prompt layering, assembly order, cache strategy | implemented |

## Reference

| File | What it covers | Status |
|------|----------------|--------|
| [glossary.md](glossary.md) | Canonical concept names | implemented |

## Contributing

Rules for contributors.

| File | What it covers | Status |
|------|----------------|--------|
| [code-style.md](contributing/code-style.md) | Naming, visibility, imports, tests, commits | implemented |
| [modules.md](contributing/modules.md) | Module boundaries, dependency rules, provider/tool/context contracts | partial |
