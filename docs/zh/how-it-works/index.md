# 工作原理

理解 kuku 的核心机制与公开运行时行为。

- [会话文件与 Rust SDK](why-kuku.md) — 可读日志、可恢复 Session 与 SDK 嵌入
- [文件原生执行](file-native-model.md) — 磁盘日志、上下文快照与故障恢复
- [Agent 执行循环](agent-loop.md) — Turn、Step、工具批次、等待与上下文交接
- [Session 机制](sessions.md) — 消息投递、后台 Job、日志压缩、上下文压缩、回退与清理
- [性能实测](performance.md) — 安装、常驻内存、重开延迟与低内存 ARM64 实测数据
- [Memory 机制](memory.md) — 全局/项目 Memory 文件与生效时机
- [Agent 与 Skill](agents-and-skills.md) — 什么时候用 Skill，什么时候拆给另一个 Agent
- [工具权限](permissions.md) — `ToolAuthorizer`、延后审批、`run_command` 与 `run_js`

初次了解内部机制，建议先看[会话文件与 Rust SDK](why-kuku.md)，再读[文件原生执行](file-native-model.md)和[Agent 执行循环](agent-loop.md)。
