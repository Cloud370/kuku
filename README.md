# kuku

[中文](docs/zh/index.md)

> The agent explained. A readable Rust agent SDK — understand the loop from source, own your sessions as files. No database, no black box.

> [!WARNING]
> kuku is in active development. APIs and file formats may change.

kuku is a terminal coding agent with a file-native architecture. No database, no server, no hidden state. Everything — config, memory, sessions, skills — is a file you can read, edit, and version-control.

## Why kuku

- **Zero infra** — no database, no server. All state lives in human-readable files under `~/.kuku/`.
- **Inspect** — config, skills, prompts, and memory are plain files. Nothing hidden.
- **No hidden state** — runtime state lives on disk, no invisible in-memory caches.
- **Cache-first design** — minimal system prompt (~3K tokens), built for maximum cache hit rate.

## Engineering Comparison

| | kuku | Claude Code | Codex | OpenCode |
|--|------|-------------|-------|----------|
| Size | **~10 MB** | ~250 MB | ~80 MB | ~50 MB |
| Deps | **~15** | ~80 | ~280 | ~100 |
| Config | 1 TOML | JSON + flags | 9 layers | 9 layers |
| Prompt | **~3K** | ~30K | ~9K | ~15K |
| Memory | Markdown | MD + YAML | SQLite + JSONL | SQLite + JSON |

> [!NOTE]
> Based on source code analysis as of May 2026. System prompt includes all tokens injected at session initialization — system instructions, tool definitions, and runtime context.

## Features

**Core**

- Agent loop (file-native)
- Tools: read, search, edit, write, run
- Skills system
- Persistent memory (human-readable)
- Subagents (isolated sessions)
- Permission system (multi-level)
- Multi-provider (Anthropic, OpenAI)
- Streaming output

**Interface**

- CLI
- HTTP Server

**Planned**

- MCP support
- Extension system
- Web UI
- Desktop app

## Quick Start

```bash
cargo install --git https://github.com/Cloud370/kuku
kuku run say hello
```

> [!TIP]
> You need an API key for your chosen provider. Set it via environment variable (`ANTHROPIC_API_KEY` or `OPENAI_API_KEY`) or in `~/.kuku/config.toml`.

## Documentation

- [Direction](docs/en/core/direction.md) — project goals and design philosophy
- [Architecture](docs/en/core/architecture.md) — system overview
- [Agent Loop](docs/en/core/agent-loop.md) — how the loop works
- [Modules](docs/en/contributing/modules.md) — crate structure
- [Code Style](docs/en/contributing/code-style.md) — conventions

## License

Licensed under either of

- MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)
- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
