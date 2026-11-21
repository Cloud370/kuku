# Architecture

这个栏目面向维护者，说明 crate 边界、host 结构、Prompt 装配和内部运行时契约。

## 页面

- [核心方向 — 先读这一页](core-direction.md) — 精简目标决策、保留的优点、非目标与下一可用结果
- [核心演进](evolution.md) — 交付阶段、实现依据与验收条件；进度按开发工作流跟踪
- [Overview](overview.md)
- [Host Apps](host-apps.md)
- [交互客户端重构计划](interactive-clients.md) — server、Rust TUI 与 WebUI 的客户端契约、交付顺序和终端测试
- [Web 客户端](web-client.md) — 浏览器工作台设计：交互模型、token 系统、投影契约与交付阶段
- [工作区执行与会话变量](workspace-execution.md) — 跨端脚本、宿主事实、命名数据及采纳条件
- [Prompt Assembly](prompt-assembly.md)
- [Module Contracts](module-contracts.md)
- [Extension Runtime](extension-runtime.md)

## 从这里开始

核心工作先读[核心方向](core-direction.md)，再读相关代码和[演进说明](evolution.md)的对应章节。现有实现结构见 [Overview](overview.md)、[Host Apps](host-apps.md) 或 [Module Contracts](module-contracts.md)。目标规划不代表实现已经改变。
