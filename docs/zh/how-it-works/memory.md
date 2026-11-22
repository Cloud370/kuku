# Memory 机制

Memory 用纯 Markdown 保存可复用的习惯、事实和线索：

- **全局 Memory**：`$KUKU_HOME/memory.md`
- **项目 Memory**：`$KUKU_HOME/p/<workspace>/memory.md`（仅作用于对应工作区）

## 文件分区

| 分区 | 用途 |
|---|---|
| `how_to_work` | 协作偏好和工作规则 |
| `what_is_true` | 影响判断的长期事实 |
| `where_to_look` | 关键文件或资源的入口指针 |

## Agent 何时看到改动

新起的 Agent 上下文会按顺序读取全局 Memory 和项目 Memory。编辑文件会影响之后新建的上下文，但已经运行的 Agent 会保留其上下文段当初接纳的版本（重开后亦然）。如果想让某个已有 Agent 立刻知道改动，直接发消息告诉它即可。

`remember_memory` 和 `forget_memory` 会修改对应文件并在 Session 里记录工具结果，你也可以随时手动改文件。[管理 Memory](../guides/manage-memory.md) · [Prompt 装配](../architecture/prompt-assembly.md)
