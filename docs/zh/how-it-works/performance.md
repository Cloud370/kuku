# 性能实测

在 Linux 实测中，`kuku` 空闲服务端占用 **16.5 MiB RSS**，SDK 的 `Session::open` 用 **157 ms** 重开了已封存的 **509 MiB** Session。

## 空闲进程

| 进程 | 采样前的就绪状态 | 空闲 RSS | 按比例计入共享页的内存（PSS） |
| --- | --- | ---: | ---: |
| `kuku server` 0.1.0 (`84bdfa23`) | HTTP 监听就绪 | **16.5 MiB** | 14.4 MiB |
| Codex CLI 0.171.0 `app-server` | WebSocket 监听就绪 | 78.0 MiB | 66.9 MiB |
| Pi 1.5.2 | RPC `get_state` 已响应 | 110.3 MiB | 107.8 MiB |
| Claude Code 2.1.283 | 流式 JSON `initialize` 已响应 | 179.5 MiB | 177.4 MiB |

每个程序都在同一台 WSL2 Linux x86-64 主机上各启动三次，取中位数。测试使用隔离的数据目录，不发送任务或模型请求，在各自就绪后一秒采样。kuku 使用带内嵌 Web UI 的 glibc release 和 `opt-level = 2`。Codex 使用发行版原生 musl CLI。Pi 使用 `--mode rpc --no-session --offline`。Claude Code 使用流式 JSON 输入输出的 print 模式，关闭会话持久化和非必要网络流量。这些数字对应表中列出的空闲模式。

## 重开大型 Session

| 指标 | 普通日志 | 已封存日志 |
| --- | ---: | ---: |
| 日志文件大小 | 509 MiB | 65 MiB |
| 重开中位数 | 2.62 s | **157 ms** |
| 重开时进程峰值 RSS | 13.7 MiB | 14.1 MiB |
| 完成消息回退时的累计峰值 RSS | 23.2 MiB | 24.7 MiB |
| 重开时的日志读取次数 | 4,073 | **12** |
| 重开时读取的日志字节数 | 509 MiB | **274 KiB** |
| 连续短轮 | 192 ms | 188 ms |
| SDK 读取最近 40 个事件（`events_before`） | 0.49 ms | 1.68 ms |

这些数字只统计 SDK 进程。实际负载的活跃 Agent、已加载上下文和工具子进程会增加内存占用。耗时来自 Linux 下三组新进程配对测试，沿用操作系统页面缓存，使用同一套体积优化 release 配置和固定的 SDK 模型适配器。读取次数来自独立的系统调用跟踪。合成历史包含重复日志，压缩效果较好。首次封存约需 3 秒。读取历史页时会解压实际访问的帧。在原子发布前后分别强制终止 Linux 进程，原有 198,454 条已提交事件均完整保留。[实测证据与条件](https://github.com/Cloud370/kuku/issues/151#issuecomment-7075736730) · [按需运行 Session 压测](../contributing/testing.md#大型会话压测)

### TUI 恢复会话

另一次 WSL2 实测使用了相同的逻辑历史和预热后的操作系统页面缓存。选中已封存的 **509 MiB** Session 后，**50 ms** 即可看到第一页对话，服务端峰值为 **20 MiB RSS**。旧版测同一份日志时耗时 **3.61 秒**，峰值为 **1.09 GiB RSS**。

服务端通过 SDK 的历史视图处理可见性和 Turn 编号，并按页读取事件。未封存的日志首次读取时需要流式扫描来构建视图。实测选中未封存的会话到显示第一页耗时 **2.15 秒**，服务端峰值为 **18.5 MiB RSS**。

## 下载和安装

每个平台发布一个压缩包。Linux 和 macOS 使用 `.tar.gz`，Windows 使用 `.zip`，Windows PowerShell 5.1 也能直接安装。清单提供大小时，脚本会显示包大小，并在交互式终端显示下载进度。脚本重试暂时的失败，并在安装前校验 SHA-256。用匹配的缓存重新安装时，无需下载压缩包。[安装 kuku](../start/install.md)

| Linux 本地格式对照 | 下载大小 | 解包中位数 |
| --- | ---: | ---: |
| gzip，发布格式 | 11.35 MiB | 151 ms |
| xz，对照格式 | 8.05 MiB | 410 ms |

两种包包含同一个 Linux glibc release 二进制。解包测试沿用操作系统页面缓存。发布使用 gzip，精简 Linux 环境也能通过常用 gzip 工具安装。
