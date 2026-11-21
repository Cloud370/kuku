# Upgrade kuku

## Replace the executable

For a development build installed with Cargo, install the current revision of the unified binary.

```bash
cargo install --locked --git https://github.com/Cloud370/kuku --force kuku-app
```

For a tagged release, choose the new release’s manifest and use the installer for your platform. [Installation](../start/install.md#install-a-published-release)

## Reconnect to the managed service

Launch the new executable as usual. For each data directory, the installed client reuses a managed service of the same build or replaces an idle service from an older release. Starting a different build of the same release selects that build. A client from an older release reports the newer running build. Use a separate data directory to run releases side by side.

When a service is busy, the TUI waits for the work to finish and connects automatically. An open TUI reports the new build when a version change disconnects its Session and keeps its own build on reconnect. Use `kuku server status` to see the running build and active work. An orderly replacement preserves committed history, waiting requests and queued inputs. [Run the server](run-the-server.md) · [Sessions](../how-it-works/sessions.md)

## Check your configuration

```bash
kuku --help
kuku config validate
```

kuku keeps settings in `config.toml`. Use `kuku config show` to inspect the selected model tier and connection. [Configuration](../start/configuration.md)
