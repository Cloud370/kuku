[English] | [中文](docs/zh/index.md)

# kuku

Turn agent runs into files you can open.

Every question, every tool call, every answer — one line in one file. `grep` it. `diff` it. Commit it. Nothing hidden, nothing locked inside an app.

## What it does

- **File-native agent loop** — everything the agent does is a line in `events.jsonl`
- **Tools** — read, search, edit, write files. Run commands. Dispatch subagents.
- **Permissions** — runtime gate. Read tools auto-allowed, commands ask, dangerous ops denied.
- **Subagents** — delegate work to isolated child sessions with constrained tools
- **Config** — define model tiers (`strong` / `balanced` / `light`) in `~/.kuku/config.toml`

## Documentation

| What | Where |
|------|-------|
| All docs | [docs/en/](docs/en/) |
| Direction & principles | [direction](docs/en/core/direction.md) |
| Architecture | [architecture](docs/en/core/architecture.md) |
| Glossary | [glossary](docs/en/glossary.md) |
| Contributing | [code style](docs/en/contributing/code-style.md) · [modules](docs/en/contributing/modules.md) |
