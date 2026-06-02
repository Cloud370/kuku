# Upgrade kuku

The current public CLI surface does not expose a documented `kuku update` subcommand.

## Upgrade by Reinstalling

If you installed with Cargo:

```bash
cargo install --git https://github.com/Cloud370/kuku --force
```

If you installed with a release script, rerun the same script for your platform.

## Review Config After Upgrades

Check your config file after upgrading:

```bash
kuku config show
kuku config validate
```

The runtime can auto-patch missing `[handoff]`, `[plugin]`, and `[update]` sections when loading older configs.

## Update Channel Settings

`config.toml` still carries update source and channel settings:

```toml
[update]
source = "github"
channel = "stable"
```

See [Config](../reference/config.md) and [Update Manifest](../reference/update-manifest.md).

## Verify the New Binary

```bash
kuku --help
```
