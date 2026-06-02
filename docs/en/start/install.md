# Install

## Prerequisites

- Rust and `cargo` if you install from source.
- An API key for at least one configured provider.
- A writable home directory, or `KUKU_HOME` set to a writable path.

## Install with Cargo

```bash
cargo install --git https://github.com/Cloud370/kuku
```

This installs the `kuku` binary from the repository.

## Install with the Release Script

Current docs also define release install scripts:

- Linux and macOS: `curl -fsSL https://kuku.run/install.sh | sh`
- Windows PowerShell: `irm https://kuku.run/install.ps1 | iex`

These scripts use the same update manifest described in [Update Manifest](../reference/update-manifest.md).

## Verify the Install

```bash
kuku --help
kuku init
```

`kuku init` creates the default runtime layout and writes `config.toml` if it does not exist yet.

## Next

1. Set provider credentials in your shell or in `config.toml`.
2. Review [Configuration](configuration.md).
3. Run [Quickstart](quickstart.md).
