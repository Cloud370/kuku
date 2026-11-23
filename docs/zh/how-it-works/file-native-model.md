# 文件原生执行

每个 Session 底层都是一份 `events.jsonl` 日志和一个保留命令输出的目录，工具直接读写你工作区里的真实项目文件。本地服务默认把 Session 存在 `$KUKU_HOME` 下（可用 `--data-dir` 切换），直接嵌入 Rust SDK 时由调用方自己指定存储根目录。[文件布局](../reference/file-layout.md)

## Agent 记住了什么

Agent 起新上下文时，kuku 会把选定的 Prompt 资源、Agent 指令、项目指令（`AGENTS.md` / `CLAUDE.md`）和 Memory 装配进来，并如实记录模型实际收到的内容。后续 Step 直接复用已提交的前缀，只追加新接纳的输入和工具结果。事后修改 `AGENTS.md` 或 `memory.md` 也不会影响还原之前的对话。哪怕两个 Agent 同时在同一个目录里干活，各自的上下文也是完全隔离的。[Prompt 装配](../architecture/prompt-assembly.md)

## 读取、所有权与恢复

客户端可以按游标拉取已提交事件，或者直接查某个 Agent 最近已落盘的快照；实时流式文本只用来展示进行中的进度。就算客户端断开，已提交的结果都稳稳留在 Session 里。

重开 Session 时会自动修复 `events.jsonl` 尾部未写完整的行，并恢复待回答的问题和待审批的权限请求。如果中断发生在外部动作派发**之后**，该动作的结果会标记为未知，避免盲目重放。

HTTP 服务（包括 `kuku` 和 `kuku run` 自动拉起的受管本地服务）替客户端持有 Session 执行权，所以关掉终端不会打断正在跑的任务；只有直接嵌入 Rust SDK 的宿主程序才在自己进程内持有 `Session`。[Session 机制](sessions.md) · [运行服务](../guides/run-the-server.md)
