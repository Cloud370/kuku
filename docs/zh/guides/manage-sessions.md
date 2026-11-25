# 管理 Session

在 TUI 里，用 `/resume` 重开当前工作区的对话，用 `/agents` 选择下条消息发给哪个 Agent。每个窗口各自维护自己的草稿。[终端界面](../reference/cli.md#终端界面)

在列表中输入文字可以筛选 Session，按 Enter 打开高亮项。打开 Session 不会发送模型请求。

## 回溯或从历史节点继续

Agent 空闲时在 TUI 里按 **Ctrl+G** 打开对话历史。比如从第 100 轮回到第 90 轮继续聊，以后再打开同一列表，依然可以选回保存下来的第 100 轮——两条分支都留在同一个 Session 里。[编辑过去的消息或接着聊](../reference/cli.md#回退与分支切换)

## 列出 Session 与 Agent

```bash
kuku list                                  # 仅当前工作区
kuku list --all                            # 所有工作区
kuku list --workspace /path/to/workspace   # 指定工作区
kuku list <session-id>                     # 查看某 Session 内的 Agent 地址、状态与排队数
```

## 继续 Session 或回复待决请求

```bash
kuku run --session <session-id> "continue"
kuku run --continue "continue"
```

加上 `--agent reviewer` 可以在同一 Session 里创建或继续另一个 Agent。新 Agent 从独立上下文开始，重开已有地址则保留它自己的上下文与模型配置。

Agent 暂停等待回答或授权时，用精确的 `request_id` 回复：

```bash
kuku reply <session-id> <request-id> --agent reviewer --answer "the requested information"
kuku reply <session-id> <request-id> --agent reviewer --allow
```

拒绝授权传 `--deny`。普通后续消息永远不会当作授权。回复带有命令 ID，支持幂等重试，过期请求 ID 会被直接拒绝。

## 读取 Session 数据

```bash
kuku show <session-id>
kuku show <session-id> --agent review
kuku show <session-id> --agent review --json
kuku events <session-id>
kuku events <session-id> --agent review
```

- `kuku show` 读取单个 Agent 最近一次持久回复。
- `kuku events` 默认展示最近 40 条已提交记录；可用 `--agent` / `--kind` 过滤，或 `--all --json` 导出完整账本。[事件查询](../reference/cli.md#kuku-events)
- Session 在其他位置时，上述命令均支持 `--workspace` 和 `--data-dir`。

## 保存与复用值

想在第一次发请求前先存一段长文本，在 TUI 里输入 `/save`，粘贴并命名即可。想留存某次命令日志，用 `/tools` 打开其输出捕获，在查看器里按 **S**。用 `/resume` 重开 Session 后，通过 `/values` 查看已保存的值，或直接把引用附到下一条消息里。[终端界面](../reference/cli.md#终端界面)

## 删除 Session

```bash
kuku delete <session-id>
```

属于其他工作区时加上 `--workspace`。

关闭 Session 会保留已提交上下文与待决请求。输入在排队期间可以从客户端撤回，一旦接纳进模型上下文就不可修改。[Session 模型](../how-it-works/sessions.md) · [CLI 参考](../reference/cli.md) · [文件布局](../reference/file-layout.md) · [事件参考](../reference/events.md)
