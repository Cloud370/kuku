# File-Native Model

kuku 将文件视为运行时事实的来源。

## The model

- `Session` 是 `$KUKU_HOME` 下的一个目录。
- `events.jsonl` 是该 Session 的持久事件日志。
- 全局和项目级 `Memory` 保存在 `memory.md` 文件中。
- 项目指令来自 `AGENTS.md` 和 `CLAUDE.md` 这样的文件。
- 权限状态和扩展包由文件承载。

不存在一个单独定义这次运行的数据库。如果某个事实重要，它必须可以从文件中恢复出来。

## What gets rebuilt

每次模型调用前，kuku 都会基于以下内容重建请求上下文：

1. Prompt 资源。
2. 项目指令。
3. 全局和项目级 `Memory`。
4. 先前已持久化的事件。
5. 当前运行时通知和目录清单。

这让执行模型在不同 Host 之间以及在进程重启后都保持稳定。

## What stays derived

有些视图只是为了方便存在，但它们不是独立状态：

- 渲染后的转录内容
- Session 摘要
- 检查输出
- UI 事件流

这些视图都来自已经存在的文件，主要是 `events.jsonl` 和当前工作区文件。

## Recovery rule

只有追加写入的事件会被信任。如果进程在某一轮中途停止，kuku 会从事件日志中最后确认的事实继续，而不是猜测发生了什么。

## Relationship to hosts

SDK 拥有这个文件承载的运行时模型。Host 应用负责展示、传输和用户交互。Host 可以是终端应用、server 或其他界面，但持久化的 Session 模型保持不变。

关于 Session 目录模型，请参见 [Sessions](sessions.md)；关于轮次执行，请参见 [Agent Loop](agent-loop.md)；关于面向维护者的上下文重建视图，请参见 [Prompt Assembly](../architecture/prompt-assembly.md)。
