# kuku

[中文](docs/zh/index.md)

> Minimal terminal coding agent, file-native at its core

kuku is a terminal coding agent with a file-native architecture. No database, no server, no hidden state. Everything — config, memory, sessions, skills — is a file you can read, edit, and version-control.

## Why kuku

- **Zero infra** — no database, no server. All state lives in human-readable files under `~/.kuku/`.
- **Inspect** — config, skills, prompts, and memory are plain files. Nothing hidden.
- **No hidden state** — runtime state lives on disk, no invisible in-memory caches.
- **Cache-first design** — minimal system prompt (~3K tokens), built for maximum cache hit rate.

## Engineering Comparison

| | kuku | Claude Code | Codex | OpenCode |
|--|------|-------------|-------|----------|
| Binary size | **~10 MB** | ~250 MB | ~80 MB | ~50 MB |
| Dependencies | **~15** | ~80 | ~280 | ~100 |
| Config | 1 TOML file | JSON + flags + feature gates | 96 fields, 9 TOML layers | 57+ fields, 9 layers |
| System prompt | **~3K tokens** | ~30K tokens | ~9K tokens | ~15K tokens |
| Memory | 1 markdown file | Markdown + YAML | SQLite + JSONL + Markdown | SQLite + JSON (no dedicated memory) |

> Based on source code analysis as of May 2026. System prompt includes all tokens injected at session initialization — system instructions, tool definitions, and runtime context.

## Features

| Feature | Status |
|---------|--------|
| File-native agent loop | Done |
| Tools (read, search, edit, write, run) | Done |
| Skills system | Done |
| Memory (persistent, human-readable) | Done |
| Subagents (isolated sessions) | Done |
| Permission system (multi-level) | Done |
| Multi-provider (Anthropic, OpenAI) | Done |
| Streaming | Done |
| CLI | Done |
| HTTP Server | Done |
| MCP support | Planned |
| Extension system | Planned |
| Web UI | Planned |
| Desktop app | Planned |

## Quick Start

```bash
cargo install --git https://github.com/Cloud370/kuku
kuku run say hello
```

## Documentation

[docs/en/](docs/en/)

## License

Licensed under either of

* MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)
* Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
