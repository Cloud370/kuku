# 更新清单

发布更新清单文件名为 `latest.json`，供安装脚本与更新频道配置读取。

## 格式

`latest.json` 采用 Tauri updater 清单格式：

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

## 字段

顶层字段：

| 字段 | 含义 |
|---|---|
| `version` | 发布版本号 |
| `notes` | 发布说明 |
| `pub_date` | 发布时间 |
| `platforms` | 按平台划分的下载条目映射 |
| `desktop` | 预留给桌面更新器的元数据 |

平台条目：

| 字段 | 含义 |
|---|---|
| `url` | 下载地址 |
| `sha256` | SHA-256 校验和 |
| `size` | 可选压缩包大小（字节），供安装脚本展示 |

## 配置示例

```toml
[update]
source = "github"
channel = "stable"

[update.sources]
mirror = "https://mirror.example.com/kuku/latest.json"
```
