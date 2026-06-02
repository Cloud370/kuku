# Update Manifest

发布 manifest 文件名为 `latest.json`。

## 作用

当前文档中，这个 manifest 用于发布安装脚本和更新 channel 配置。

## 格式

`latest.json` 遵循 Tauri updater manifest 结构。

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

## 必需的顶层键

| Key | Meaning |
|---|---|
| `version` | 发布版本 |
| `notes` | 发布说明 |
| `pub_date` | 发布时间戳 |
| `platforms` | 按平台划分的下载映射 |

`desktop` 预留给桌面更新器元数据。

## 平台条目

每个平台条目包含：

| Key | Meaning |
|---|---|
| `url` | 下载 URL |
| `sha256` | SHA-256 校验和 |

## Config 链接

`config.toml` 通过以下配置选择 manifest 来源：

```toml
[update]
source = "github"
channel = "stable"

[update.sources]
mirror = "https://mirror.example.com/kuku/latest.json"
```
