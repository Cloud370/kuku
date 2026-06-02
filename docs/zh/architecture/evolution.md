# Evolution

本页记录当前架构方向，以及已经完成或仍计划中的主要实现阶段。

## Stable direction

有几项设计选择被认为应保持稳定：

- 以文件为后盾的运行时事实
- 只追加的事件持久化
- host app 与 SDK 内部分离
- 把 Skill 作为原生指令加载方式
- 把扩展视为外部边界，而不是核心运行时特性

## Implemented path so far

| Phase | What | Layer | Status |
|-------|------|-------|--------|
| 1 | Skills and registry loading | SDK | implemented |
| 2 | wire-facing `UiEvent` shape | SDK | implemented |
| 3 | NDJSON wire serialization | SDK | implemented |
| 4 | HTTP server host | host | implemented |
| 5 | web host | host | implemented |
| 6 | package and hook runtime | SDK | implemented |

## Planned path

| Next | Purpose |
|------|---------|
| MCP-backed external tool sources | 通过扩展边界增加非核心 Tool provider |
| Tauri host | 围绕 server 运行时构建的桌面壳层 |

## Design pressure to watch

- 让 provider 逻辑继续与 Session 和权限逻辑隔离
- 随着更多运行时通知出现，保持 Prompt 装配稳定
- 除非属于基础运行时行为，否则让扩展点保持在核心 loop 之外
- 让公开的心智模型文档继续与维护者内部文档分离

如果未来某个改动主要影响用户可见行为，请在 `how-it-works/` 中记录。如果它主要影响 crate 边界或内部归属，请在这里的 `architecture/` 中记录。
