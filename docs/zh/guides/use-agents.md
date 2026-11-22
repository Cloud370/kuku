# 使用 Agent

## 把任务交给第二个 Agent

先启动一个 Session 并记下输出的 ID，在同一工作目录运行后续命令（把 `SESSION_ID` 替换为实际 ID）：

```bash
kuku run "Inspect this project and summarize the test setup"
kuku run --session SESSION_ID --agent reviewer "Read the test files and identify a missing case"
kuku list SESSION_ID
kuku show SESSION_ID --agent reviewer
```

第二条命令会按需创建 `reviewer`，之后对同一 Session 和地址再次执行 `kuku run` 会延续它自己的上下文。新 Agent 从干净上下文启动，委派任务时记得把相关文件路径或已有结论一起带给它。

如果要把一段保留的日志或大块结果转给其他 Agent，让发送方先命名保存，再把名称放进 `send_message` 的 `values` 参数，接收方会在任务里拿到精确引用。[命名会话值](../reference/tools.md#命名会话值)

## 复用 Markdown 预设

把 Agent 预设放到 `<workspace>/.kuku/agents/`（项目级）或 `~/.kuku/agents/`（个人级），同名时项目级优先。查看已发现的预设：

```bash
kuku agents list
kuku agents show NAME
```

运行期间，模型可以用 `create_agent` 指定新地址和 `preset`，用 `send_message` 派发任务，用 `list_agents` 查看对方状态和最近落盘的输出。如果某次运行不想让模型创建或调度子 Agent，加上 `--no-agents` 即可。[Agent 格式](../reference/agent-format.md) · [工具参考](../reference/tools.md) · [管理 Session](manage-sessions.md)
