# 文件原生执行

每个 Session 都有自己的 `events.jsonl` 日志和命令输出保留目录。工具读写的项目文件就在工作目录中。本地服务默认使用 kuku 数据目录存放 Session。本地客户端可以用 `--data-dir` 选择其他服务数据目录。嵌入方自行指定存储根目录。[文件布局](../reference/file-layout.md)

## Agent 记住什么

初始上下文包括选定的 Prompt 资源、Agent 指令、项目指令和 Memory。SDK 记录模型实际收到的内容。后续 Step 复用已提交的前缀，加入新接纳的输入和工具结果。运行后编辑 `AGENTS.md` 或 `memory.md`，仍然可以还原之前的对话。两个 Agent 即使在同一目录工作，也各有自己的上下文。[Prompt 装配](../architecture/prompt-assembly.md)

## 读取与恢复

客户端可以按游标读取已提交的事件，也可以查看 Agent 最近已落盘的结果。实时文本片段用来展示进度。观察者断开后，仍可从 Session 读取已提交结果。重开时修复未写完整的日志尾部，并恢复待回答的问题或授权请求。外部动作派发后发生中断，其结果可能仍未知。

HTTP server 为连接它的客户端持有执行，关闭客户端后服务端任务仍能继续。当前本地 CLI 在运行期间直接持有自己的 Session。[Session](sessions.md) · [运行服务](../guides/run-the-server.md)
