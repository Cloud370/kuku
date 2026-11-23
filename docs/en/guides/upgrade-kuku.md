# Upgrade kuku

## Update the binary

If you built from source with Cargo:

```bash
cargo install --locked --git https://github.com/Cloud370/kuku --force kuku-app
```

For a tagged release, point your platform installer at the new release's `latest.json`. [Install a published release](../start/install.md#install-a-published-release)

## Managed service handoff

Launch the new binary as usual. Per `KUKU_HOME`:

- Same build: reuses the running managed service.
- Newer release (or different build of the same release): replaces the managed service once it is idle.
- Older client against a newer running service: reports the newer running build instead of downgrading.

To run two versions side by side, point each one at its own `KUKU_HOME`.

If the service is busy during an upgrade, a newly launched TUI waits until active work finishes and connects automatically. An already-open TUI notifies you if a version swap disconnects its Session and stays on its own binary when reconnecting. Check active work with `kuku server status`. Normal handoff preserves committed history, pending requests, and queued inputs. [Run the server](run-the-server.md) · [Sessions](../how-it-works/sessions.md)

## Verify your config

```bash
kuku --help
kuku config validate
```

Run `kuku config show` to double-check your active tier and connection. [Configuration](../start/configuration.md)
