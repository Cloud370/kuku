# 运行任务

## 单次执行

在任意目录下都可以直接运行，不需要 Git 仓库：

```bash
kuku run "check this project"
kuku run --tier strong "review this diff"
kuku run --connection <configured-name> --model <model-id> "task"
```

需要针对其他目录运行时加上 `--workspace <path>`。默认本地策略允许文件和命令工具，交付前请自行核对实际改动的文件和命令结果。完整参数见 [CLI 参考](../reference/cli.md)。

## 交互式 TUI

```bash
kuku
```

输入提示词后按 **Enter** 发送。Agent 执行期间：

- **Enter**：发送补充输入，在下一个安全的模型请求边界生效（如果当前 Turn 先结束，则自动进入下一轮）。
- **Alt+Enter**：排队等到下一轮（Turn）再处理。
- **Shift+Enter** 或 **Ctrl+J**：在输入框内换行。
- **Esc**：打断当前选中的 Agent 并保留草稿（在菜单里按 Esc 则关闭菜单）。

用 `/new` 起一份新 Session 草稿，`/resume` 打开当前工作区的已有 Session，`/agents` 切换下条消息的接收对象。关掉终端窗口不影响后台服务继续跑完已接下的工作。[终端界面](../reference/cli.md#终端界面)

## 继续之前的任务

```bash
kuku run --session <session-id> "continue"
kuku run --continue "continue"
```

## 查看结果

- `kuku show <session-id>`：查看最近一次持久回复
- `kuku events <session-id>`：查看持久事件日志
- `kuku list`：列出当前工作区的近期 Session

## 脚本与详细输出模式

- `--json`：输出一行最终 JSON，包含持久状态与模型用量
- `--stream-json`：实时输出 JSON 行进度，最后一条与 `--json` 结构一致
- `--verbose`：人类可读的详细输出（含状态、配置与用量）
- `--raw`：仅输出纯文本

完整字段定义见 [CLI 参考](../reference/cli.md#kuku-run)。用量由 Provider 报告，字段缺失不代表零消耗。

如果 Agent 因提问或权限确认暂停，JSON 输出会返回 `event: "waiting"` 及精确的 `request_id`。使用相同的工作区和数据目录，通过 `kuku reply <session-id> <request-id> --answer "..."`、`--allow` 或 `--deny` 回复即可。[管理 Session](manage-sessions.md)

按 `Ctrl-C` 会等待其子命令清理完毕。Session 日志和保留输出在关闭重开后依然完整。[文件布局](../reference/file-layout.md) · [事件参考](../reference/events.md)
