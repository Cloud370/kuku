# Update System

## How It Works

kuku supports self-update via `kuku update`. The update system fetches a `latest.json` manifest from a configured source, compares versions, downloads the platform-appropriate archive, verifies SHA256, and replaces the binary.

## Manifest Format

`latest.json` follows the [Tauri updater format](https://v2.tauri.app/plugin/updater/):

```json
{
  "version": "0.1.0",
  "notes": "...",
  "pub_date": "2026-05-31T00:00:00.000Z",
  "platforms": {
    "linux-x86_64": { "url": "...", "sha256": "..." },
    "darwin-aarch64": { "url": "...", "sha256": "..." },
    "windows-x86_64": { "url": "...", "sha256": "..." }
  },
  "desktop": {}
}
```

## Sources

Built-in: GitHub releases. Users can add custom sources in config.toml:

```toml
[update]
source = "github"
channel = "stable"

[update.sources]
mirror = "https://mirror.example.com/kuku/latest.json"
```

Priority: `--source` arg > config.toml > built-in default.

## Install Scripts

- Linux/macOS: `curl -fsSL https://kuku.run/install.sh | sh`
- Windows: `irm https://kuku.run/install.ps1 | iex`

Scripts use the same manifest and cache flow as `kuku update`.

## Cache

Downloaded archives are cached in `{kuku_home}/cache/`. Installed binary lives in `{kuku_home}/bin/`.
