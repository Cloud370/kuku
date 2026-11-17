# kuku

[English Docs](docs/en/index.md) | [中文文档](docs/zh/index.md) | [Docs Portal](docs/index.md)

> A file-native Rust agent SDK. Sessions, Memory, Skills, and config stay on disk as readable files.

> [!WARNING]
> kuku is in testing. APIs and file formats may change.

Open `kuku` for interactive work in your terminal or use `kuku run` for scripts. The server keeps accepted work running while you switch terminals or open the browser. Embed the same runtime directly with the [Rust SDK](docs/en/reference/sdk.md).

## Why kuku

- File-native runtime: sessions, Memory, config, permissions, and Skills live on disk.
- Readable architecture: the SDK owns runtime facts; host apps own presentation and transport.
- Each Agent has its own address, inbox and context within a Session.
- Each model request extends a committed context prefix. Reopening an Agent preserves the instructions and tool results its model already saw.

## Quick Start

For a fresh install, set `ANTHROPIC_API_KEY` in your shell for the default Anthropic route, then run from your project directory.

```bash
cargo install --locked --git https://github.com/Cloud370/kuku kuku-app
kuku init
kuku
```

Enter `Inspect the files in this directory` and press Enter. For OpenAI or an existing configuration, follow the [Quickstart](docs/en/start/quickstart.md). Install a release bundle to use `kuku web` with the embedded browser interface. [Install](docs/en/start/install.md)

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
- Shared client and terminal interface: [`crates/kuku-client/`](crates/kuku-client/) and [`crates/kuku-tui/`](crates/kuku-tui/)
- Unified binary: [`apps/kuku/`](apps/kuku/)

## License

Licensed under either of:

- MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)
- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
