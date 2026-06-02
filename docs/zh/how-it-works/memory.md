# Memory

`Memory` 是存储在 markdown 文件中的长期背景上下文。

它不是数据库、向量索引，也不是隐藏在 Host 内部的特性。

## Two layers

| Scope | Path | Loaded |
|-------|------|--------|
| Global | `$KUKU_HOME/memory.md` | 每个 Session |
| Project | `$KUKU_HOME/p/<workspace>/memory.md` | 该工作区中的 Session |

全局 `Memory` 会先于项目级 `Memory` 加载。

## Structure

`memory.md` 使用三个固定分区：

| Section | Purpose |
|---------|---------|
| `how_to_work` | 协作偏好和工作规则 |
| `what_is_true` | 会影响决策的长期事实 |
| `where_to_look` | 指向外部资源的线索 |

这个文件保持为普通 markdown。不需要 id、时间戳或额外 schema。

## How it changes

运行时会暴露专门的 Memory Tool，用于追加或移除条目。Memory 变更会像其他 Tool 结果一样通过同一个事件日志写入，因此下一轮重建时能看到更新后的文件。

用户也可以直接编辑这些文件。

## What belongs in Memory

`Memory` 适合保存那些应当跨 Session 影响行为的信息，例如：

- 用户偏好
- 持久的项目约束
- 重要的外部线索

不要把以下内容放进去：

- 临时任务状态
- 已经在项目指令中定义的事实
- secret 或凭证
- 不确定的猜测

## Drift notices

如果被跟踪的文件在两轮之间发生变化，kuku 会向运行时上下文注入一条系统通知。该通知只说明文件承载的上下文发生了变化；它不会自动重新插入新的文件内容。

被跟踪的 baseline 来源包括：

- 项目指令文件
- 全局和项目级 `Memory`
- 成功的整文件 `read_file` 快照

部分读取不会创建或刷新被跟踪的 baseline。Tool 形式的写入只会刷新该路径上已经存在的被跟踪 baseline。

## Mental model

`Memory` 是共享的背景上下文，不是当前的对话转录。Session 历史保存在 `events.jsonl` 中；`Memory` 保存在 markdown 文件中，并作为前置上下文加载。

关于 `Memory` 如何进入请求的维护者视图，请参见 [Prompt Assembly](../architecture/prompt-assembly.md)。
