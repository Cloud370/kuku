# Memory

Memory 用 Markdown 保存可复用的指令、事实和参考资料。全局 Memory 位于 `$KUKU_HOME/memory.md`。项目 Memory 位于 `$KUKU_HOME/p/<workspace>/memory.md`，作用于对应工作目录。

## 文件分区

| 分区 | 用途 |
|---|---|
| `how_to_work` | 协作偏好和工作规则。 |
| `what_is_true` | 影响决策的长期事实。 |
| `where_to_look` | 相关文件或其他资源的入口。 |

## Agent 何时看到改动

新的 Agent 上下文依次读取全局和项目 Memory。编辑文件会改变新上下文能加载的内容。已有 Agent 保留其上下文段已经接纳的版本，重开后也是如此。希望该 Agent 现在就知道改动时，可以直接给它发送新信息。

`remember_memory` 和 `forget_memory` 会编辑对应文件，并在 Session 中记录工具结果。用户也可以直接编辑文件。[管理 Memory](../guides/manage-memory.md) · [Prompt 装配](../architecture/prompt-assembly.md)
