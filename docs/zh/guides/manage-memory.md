# Manage Memory

Memory 是存储在磁盘上的长期上下文。

## Know the Two Scopes

- 全局 Memory 适用于所有 Session。
- 项目 Memory 只适用于一个工作区。

路径见 [File Layout](../reference/file-layout.md)。

## Let the Agent Update Memory

运行时暴露了两个专用 Tool：

- `remember_memory`
- `forget_memory`

它们的准确参数定义见 [Tools](../reference/tools.md)。

## Review Memory Files

当你想以文本形式检查或编辑时，可以直接读取当前 Memory 文件：

- global: `$KUKU_HOME/memory.md`
- project: `$KUKU_HOME/p/<workspace>/memory.md`

## Keep Memory Small

好的 Memory 条目应当是稳定的指导、持久的事实，以及会影响后续决策的线索。

不要把以下内容放进 Memory：

- 临时任务笔记
- Session 转录
- secret
- 已经由 `AGENTS.md` 或 `CLAUDE.md` 强制定义的事实

## Related Pages

- [File Layout](../reference/file-layout.md)
- [Glossary](../reference/glossary.md)
