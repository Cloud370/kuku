# 使用 Agent

## 把任务交给第二个 Agent

先启动一个 Session，从输出中复制它的 ID。以下命令都在同一工作目录运行，并把 `SESSION_ID` 换成该 ID。

```bash
kuku run "Inspect this project and summarize the test setup"
kuku run --session SESSION_ID --agent reviewer "Read the test files and identify a missing case"
kuku list SESSION_ID
kuku show SESSION_ID --agent reviewer
```

第二条命令在需要时创建 `reviewer`。之后向同一 Session 和地址再次执行 `kuku run`，会延续它自己的历史。新 Agent 有独立上下文，委派时应把相关文件或已有发现发给它。

## 复用预设

项目预设放在 `<workspace>/.kuku/agents/`，个人预设放在 `~/.kuku/agents/`。同名时优先使用项目定义。用 `kuku agents list` 查看目录，用 `kuku agents show NAME` 查看某个预设。[Agent 格式](../reference/agent-format.md)

运行中，模型可以调用 `create_agent` 指定新地址和 `preset`，再用 `send_message` 发送任务。通过 `list_agents` 查看接收方状态和最近已落盘的结果。使用 `--no-agents` 可以关闭当前运行的模型协作工具。[工具参考](../reference/tools.md) · [管理 Session](manage-sessions.md)
