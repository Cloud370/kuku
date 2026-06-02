# Why kuku

kuku 是一个用于 Agent 执行的 Rust SDK，运行时状态保存在普通文件中。

## Core idea

- `Session` 是一个目录。
- 事件日志只追加，不覆盖。
- 项目指令、`Memory`、权限、Agent 和 Skill 都由文件承载。
- Host 会在每次模型调用前，基于这些文件重建上下文。

这样得到的是一个可以用常规工具检查的运行时。你可以读取事件日志、比较一次运行的差异，或将周边状态提交到 git。

## What kuku is

- 一个供 Host 应用构建其上的 SDK。
- 一个将 Agent 事实持久化到磁盘的运行时。
- 一个供 CLI、server 和其他 Host 共享的执行模型。

## What kuku is not

| If you want | kuku is not that |
|-------------|-------------------|
| 一个隐藏的 Session 存储 | 状态是磁盘上的文件。 |
| 一个单一的聊天应用产品 | Host 是基于 SDK 构建的独立应用。 |
| 一个以插件为核心的系统 | 核心循环保持精简，扩展附着在其周围。 |

## Why the file-native model matters

- 无需专用工具也能检查状态。
- 恢复依赖已持久化的事件，而不是内存中的假设。
- 不同 Host 应用可以用不同方式呈现同一组运行时事实。
- 运行规则靠近项目本身，而不是封装在某一个 Host 内部。

接下来请阅读 [File-Native Model](file-native-model.md)，然后是 [Agent Loop](agent-loop.md)。如果想看面向维护者的结构，请参见 [Architecture Overview](../architecture/overview.md)。
