# Install kuku

## Build the testing version

Install Rust and Cargo, then select the workspace package that provides the `kuku` executable.

```bash
cargo install --locked --git https://github.com/Cloud370/kuku kuku-app
kuku --help
```

The default build includes the CLI and HTTP server. To build the browser UI from a checkout, use the build instructions in the [CLI reference](../reference/cli.md#kuku-server-and-kuku-web).

## Install a published release

For a tagged release, choose its `latest.json` asset from the release list. From a checkout, pass its URL to `sh scripts/install.sh --source MANIFEST_URL` on x86-64 Linux or Apple Silicon macOS, or to `.\scripts\install.ps1 -Source MANIFEST_URL` on x86-64 Windows. The default installer URL selects a stable release. Use Cargo above to try the development tree. [Releases](https://github.com/Cloud370/kuku/releases) · [Unix installer](../../../scripts/install.sh) · [Windows installer](../../../scripts/install.ps1) · [Update manifest](../reference/update-manifest.md)

## Run the first task

Use a writable home or set `KUKU_HOME` to a writable directory. The quickstart initializes the config once and selects a provider for the first task. [Quickstart](quickstart.md)
