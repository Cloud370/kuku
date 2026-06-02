# kuku

[English Docs](docs/en/index.md) | [中文文档](docs/zh/index.md) | [Docs Portal](docs/index.md)

> A file-native Rust agent SDK. Sessions, Memory, Skills, and config stay on disk as readable files.

> [!WARNING]
> kuku is in active development. APIs and file formats may change.

kuku is the runtime behind a terminal coding agent and related host apps. It keeps runtime state in files instead of a database or hidden process state, so you can inspect what happened, diff it, and version it.

## Why kuku

- File-native runtime: sessions, Memory, config, permissions, and Skills live on disk.
- Readable architecture: the SDK owns runtime facts; host apps own presentation and transport.
- Low hidden state: every turn is rebuilt from files and persisted events.
- Cache-first prompt design: small stable prelude, dynamic state added only where needed.

## Quick Start

```bash
cargo install --git https://github.com/Cloud370/kuku
kuku run say hello
```

Set a provider API key with `ANTHROPIC_API_KEY` or `OPENAI_API_KEY`, or configure it in `~/.kuku/config.toml`.

## Docs

- Start in English: [docs/en/index.md](docs/en/index.md)
- 从中文开始: [docs/zh/index.md](docs/zh/index.md)
- Language selector: [docs/index.md](docs/index.md)

Recommended path:

1. [Install](docs/en/start/install.md)
2. [Quickstart](docs/en/start/quickstart.md)
3. [Configuration](docs/en/start/configuration.md)
4. [Guides](docs/en/guides/index.md)
5. [Reference](docs/en/reference/index.md)

## Repo Map

- SDK: [`crates/kuku/`](crates/kuku/)
- CLI: [`crates/kuku-cli/`](crates/kuku-cli/)
- Server: [`crates/kuku-server/`](crates/kuku-server/)
- Unified binary: [`apps/kuku/`](apps/kuku/)

## License

Licensed under either of:

- MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)
- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
