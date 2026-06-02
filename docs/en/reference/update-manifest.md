# Update Manifest

The release manifest file is named `latest.json`.

## Role

Current docs use this manifest for release install scripts and update-channel configuration.

## Format

`latest.json` follows the Tauri updater manifest shape.

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

## Required Top-Level Keys

| Key | Meaning |
|---|---|
| `version` | Release version |
| `notes` | Release notes |
| `pub_date` | Publication timestamp |
| `platforms` | Per-platform download map |

`desktop` is reserved for desktop updater metadata.

## Platform Entries

Each platform entry contains:

| Key | Meaning |
|---|---|
| `url` | Download URL |
| `sha256` | SHA-256 checksum |

## Config Link

`config.toml` selects the manifest source with:

```toml
[update]
source = "github"
channel = "stable"

[update.sources]
mirror = "https://mirror.example.com/kuku/latest.json"
```
