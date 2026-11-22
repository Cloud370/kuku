# kuku

> A fast, low-memory, file-native agent runtime and Rust SDK.

kuku stays light enough that a 371 MiB RAM ARM64 board can open and continue a 1,000+ turn conversation in the terminal while paging through older history. [Performance measurements](docs/en/how-it-works/performance.md#low-memory-arm64-device)

[English Docs](docs/en/index.md) | [中文文档](docs/zh/index.md) | [Docs Portal](docs/index.md)

> [!WARNING]
> kuku is still pre-1.0. Until the first stable release, compatibility covers only the current SDK, CLI, server API, and Session format. Updates may break existing integrations or make older Sessions unreadable.

## Quick Start

For a fresh install with the default Anthropic route, export `ANTHROPIC_API_KEY` and run from your project directory:

```bash
cargo install --locked --git https://github.com/Cloud370/kuku kuku-app
kuku init
kuku
```

Type `Inspect the files in this directory` and press Enter. For OpenAI or an existing config, see [Quickstart](docs/en/start/quickstart.md). To use `kuku web` with the embedded browser UI, install a release bundle instead of building from `cargo install` ([Install](docs/en/start/install.md)).

## Documentation

Recommended reading order ([中文文档](docs/zh/index.md)):

1. [Install](docs/en/start/install.md)
2. [Quickstart](docs/en/start/quickstart.md)
3. [Configuration](docs/en/start/configuration.md)
4. [Guides](docs/en/guides/index.md)
5. [Reference](docs/en/reference/index.md)

## Repo Map

- SDK: [`crates/kuku/`](crates/kuku/)
- CLI: [`crates/kuku-cli/`](crates/kuku-cli/)
- Server: [`crates/kuku-server/`](crates/kuku-server/)
- HTTP client and terminal UI: [`crates/kuku-client/`](crates/kuku-client/), [`crates/kuku-tui/`](crates/kuku-tui/)
- Web UI: [`apps/web/`](apps/web/)
- Unified binary: [`apps/kuku/`](apps/kuku/)

## License

Licensed under either of:

- MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)
- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
