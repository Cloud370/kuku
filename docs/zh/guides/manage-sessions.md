# 管理 Session

每个 Session 在 kuku home 下拥有一个目录和一份账本。

在终端界面用 `/resume` 查找当前工作区的 Session，用 `/agents` 切换接收者。各窗口保留各自的草稿。一个客户端回复问题或授权后，观察该 Agent 的其他客户端可以看到结果。[终端界面命令](../reference/cli.md#终端界面)

## 列出 Session

只列当前工作区：

```bash
kuku list
```

所有工作区：

```bash
kuku list --all
```

显式指定工作区：

```bash
kuku list --workspace /path/to/workspace
```

## 列出一个 Session 中的 Agent

```bash
kuku list <session-id>
```

显示每个稳定 Agent 地址、执行状态和待接纳输入数量。

## 继续 Session

继续 `main` Agent：

```bash
kuku run --session <session-id> "continue"
```

或继续最近的 Session：

```bash
kuku run --continue "continue"
```

使用 `--agent reviewer` 创建或继续同一 Session 中的另一个 Agent。新 Agent 默认拥有独立上下文；重开已有地址保留其自身上下文和模型配置。

等待中的 Agent 需要精确的待决请求：

```bash
kuku reply <session-id> <request-id> --agent reviewer --answer "the requested information"
kuku reply <session-id> <request-id> --agent reviewer --allow
```

使用 `--deny` 拒绝授权。普通后续 Prompt 不会成为授权。回复命令标识支持幂等重试，过期请求标识会被拒绝。

## 读取 Session 数据

```bash
kuku show <session-id>
kuku show <session-id> --agent review
kuku show <session-id> --agent review --json
kuku events <session-id>
kuku events <session-id> --agent review
```

- `kuku show` 读取一个 Agent 的最后持久响应。
- `kuku events` 读取持久账本事实。
- 省略 `--agent` 检查完整账本。
- 使用 `--after <id> --limit <n> --json` 读取有界持久页，默认上限为 100 条。
- Session 位于其他位置时，检查命令接受 `--workspace` 和 `--data-dir`。

## 保存和复用值

要在首次模型请求前保存长文本，可在终端打开 `/save`，粘贴后为文本命名。要保留命令日志，可用 `/tools` 打开其捕获，在查看器中按 S。使用 `/resume` 重开 Session 后，可用 `/values` 读取保存值，或将保存的引用附到下一次输入。见[终端命令](../reference/cli.md#终端界面)。

## 删除 Session

```bash
kuku delete <session-id>
```

属于另一个工作区时添加 `--workspace`。

关闭 Session 会保留已提交上下文和待决请求。输入在排队期间可从交互客户端撤回，接纳到模型上下文后不可修改。[Session](../how-it-works/sessions.md)

## 相关页面

- [CLI](../reference/cli.md)
- [文件布局](../reference/file-layout.md)
- [事件](../reference/events.md)
