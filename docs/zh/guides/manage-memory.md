# 管理 Memory

Memory 是以纯 Markdown 存在磁盘上的长期上下文，会自动注入后续的 Session。

## 两种作用域

- **全局 Memory**：`$KUKU_HOME/memory.md` — 对所有 Session 生效。
- **项目 Memory**：`$KUKU_HOME/p/<workspace>/memory.md` — 仅对单个工作区生效。

这两个文件都是普通文本，随时可以直接打开查看或手动编辑。[文件布局](../reference/file-layout.md)

## 让 Agent 自动维护 Memory

运行时提供两个内置工具：

- `remember_memory`
- `forget_memory`

具体参数定义见 [工具参考](../reference/tools.md)。

## 保持精简

Memory 只适合放稳定的习惯偏好、长期成立的项目事实，以及真正影响后续判断的线索。不要往里塞：

- 一次性任务笔记
- Session 对话记录
- 密钥或凭据
- 已经写在 `AGENTS.md` 或 `CLAUDE.md` 里的规则

延伸阅读：[Memory 机制](../how-it-works/memory.md) · [术语表](../reference/glossary.md)
