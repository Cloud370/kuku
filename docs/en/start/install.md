# Install kuku

## Build from source

With Rust and Cargo installed, install the `kuku` binary from the workspace package:

```bash
cargo install --locked --git https://github.com/Cloud370/kuku kuku-app
kuku --help
```

A default source build includes the TUI, CLI commands, and HTTP server. To use `kuku web`, install a release bundle (which embeds the Web UI) or [build the browser assets from a checkout](../reference/cli.md#kuku-server-and-kuku-web).

## Install a published release

Pick the `latest.json` asset URL from a tagged [GitHub Release](https://github.com/Cloud370/kuku/releases), then run the installer from a checkout:

- Linux (x86-64, ARM64) and macOS (Apple Silicon):
  ```bash
  sh scripts/install.sh --source MANIFEST_URL
  ```
- Windows (x86-64):
  ```powershell
  .\scripts\install.ps1 -Source MANIFEST_URL
  ```

Without `--source` / `-Source`, the installer targets the default stable release URL. Use `cargo install` above to track the development tree. [Unix installer](../../../scripts/install.sh) · [Windows installer](../../../scripts/install.ps1) · [Update manifest](../reference/update-manifest.md)

The installer prints the download size and progress in interactive terminals, retries transient failures, verifies SHA-256 checksums, and reuses matching cached archives on reinstall. [Download and installation measurements](../how-it-works/performance.md#download-and-installation) · [Low-memory ARM64 measurements](../how-it-works/performance.md#low-memory-arm64-device)

On Linux and macOS, the installer adds the install directory to your shell profile. If your current terminal does not have it on `PATH` yet, run the activation command it prints before starting `kuku`.

## Next step

Make sure your home directory is writable (or point `KUKU_HOME` to a writable directory), then follow the [Quickstart](quickstart.md).
