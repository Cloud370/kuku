# Update manifest

Release manifests are published as `latest.json` and used by install scripts and update-channel configuration.

## Format

`latest.json` follows the Tauri updater manifest shape:

```json
{
  "version": "0.1.0",
  "notes": "...",
  "pub_date": "2026-05-31T00:00:00.000Z",
  "platforms": {
    "linux-x86_64": { "url": "...", "sha256": "...", "size": 12345678 },
    "linux-aarch64": { "url": "...", "sha256": "...", "size": 12345678 },
    "darwin-aarch64": { "url": "...", "sha256": "...", "size": 12345678 },
    "windows-x86_64": { "url": "...", "sha256": "...", "size": 12345678 }
  },
  "desktop": {}
}
```

## Keys

Top-level keys:

| Key | Meaning |
|---|---|
| `version` | Release version |
| `notes` | Release notes |
| `pub_date` | Publication timestamp |
| `platforms` | Per-platform download map |
| `desktop` | Reserved for desktop updater metadata |

Platform entries:

| Key | Meaning |
|---|---|
| `url` | Download URL |
| `sha256` | SHA-256 checksum |
| `size` | Optional archive size in bytes (shown by installers) |

## Config example

```toml
[update]
source = "github"
channel = "stable"

[update.sources]
mirror = "https://mirror.example.com/kuku/latest.json"
```
