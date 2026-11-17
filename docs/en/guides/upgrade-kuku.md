# Upgrade kuku

## Replace the executable

For a development build installed with Cargo, install the current revision of the unified binary.

```bash
cargo install --locked --git https://github.com/Cloud370/kuku --force kuku-app
```

For a tagged release, choose the new release’s manifest and use the installer for your platform. [Installation](../start/install.md#install-a-published-release)

## Check your configuration

```bash
kuku --help
kuku config validate
```

kuku keeps settings in `config.toml`. Use `kuku config show` to inspect the selected model tier and connection. [Configuration](../start/configuration.md)
