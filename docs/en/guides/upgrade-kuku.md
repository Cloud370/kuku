# Upgrade kuku

## Replace the executable

For a development build installed with Cargo, install the current revision of the unified binary.

```bash
cargo install --locked --git https://github.com/Cloud370/kuku --force kuku-app
```

For a tagged release, choose the new release’s manifest and use the installer for your platform. [Installation](../start/install.md#install-a-published-release)

## Reconnect to the managed service

Replacing the installed client leaves the running service and its work active. If the client and server protocol versions match, open the Session from the new client. Use `kuku server status` to see the running build and active work.

If the client reports a protocol mismatch, wait for active work and tool cleanup to settle, then run `kuku server restart` using the new binary. A busy service reports what is still active so you can retry later. The orderly restart releases Session writers and preserves committed history, waiting requests and queued inputs. [Run the server](run-the-server.md) · [Sessions](../how-it-works/sessions.md)

## Check your configuration

```bash
kuku --help
kuku config validate
```

kuku keeps settings in `config.toml`. Use `kuku config show` to inspect the selected model tier and connection. [Configuration](../start/configuration.md)
