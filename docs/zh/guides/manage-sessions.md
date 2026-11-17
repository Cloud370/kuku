# 管理 Session

每个 Session 在 kuku home 下拥有一个目录和一份账本。

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

## 删除 Session

```bash
kuku delete <session-id>
```

属于另一个工作区时添加 `--workspace`。

关闭 Session 保留已提交上下文和待决请求。已经接纳至 Provider 可见上下文的输入不可变；交互宿主不再提供原地转录回滚。

## 相关页面

- [CLI](../reference/cli.md)
- [文件布局](../reference/file-layout.md)
- [事件](../reference/events.md)
