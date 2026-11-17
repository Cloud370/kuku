# 执行任务

适用于 kuku 已经安装并配置完成的情况。

## 开始新任务

执行一次性任务：

```bash
kuku run "check this project"
```

显式选择 Tier：

```bash
kuku run --tier strong "review this diff"
```

需要精确端点与模型时，使用 `kuku run --connection <configured-name> --model <model-id> "task"`。可以在普通目录运行，也可传 `--workspace <path>`，无需初始化 Git。默认本地策略允许文件和命令工具；只有检查了实际文件和命令结果，才能认定任务成功。

精确参数见 [CLI](../reference/cli.md)。

## 使用交互模式

从工作区启动全屏终端客户端：

```bash
kuku
```

输入提示词后按 Enter 发送。Agent 忙时，后续输入会在下一个安全的模型请求边界接纳。如果当前轮先结束，输入会进入下一轮。按 Alt+Enter 排到下一轮。按 Shift+Enter 或 Ctrl+J 在草稿中换行。

用 `/new` 为新 Session 打开空白草稿，或用 `/resume` 打开当前工作区的 Session。用 `/agents` 选择下一条输入的接收者。在对话中按 Esc 会打断所选执行并保留草稿。在菜单中按 Esc 会关闭菜单。终端窗口可以断开，服务会继续执行。[终端界面命令](../reference/cli.md#终端界面)

## 继续现有工作

继续指定 Session：

```bash
kuku run --session <session-id> "continue"
```

或继续最近的 Session：

```bash
kuku run --continue "continue"
```

## 检查输出

- `kuku show <session-id>`：最终响应。
- `kuku events <session-id>`：持久事件日志。
- `kuku list`：当前工作区近期 Session。

## 调整输出模式

供其他工具或脚本消费时可使用：

- `--json`：一行最终 JSON。
- `--stream-json`：实时 JSON 行及最终用量。
- `--verbose`：含持久状态、配置与用量的可读输出。
- `--raw`：纯文本。

### JSON 输出结构

`--json` 返回所选 Agent 的持久结果。`--stream-json` 另含瞬时进度，最终对象相同。字段和控制选项统一定义于 [CLI 参考](../reference/cli.md#kuku-run)。用量由 Provider 报告，缺失报告不能当作零消耗。

持久问题或授权返回 `event: "waiting"` 和精确请求标识。使用 `kuku reply <session-id> <request-id> --answer "answer"`、`--allow` 或 `--deny` 回复，并保留相同工作区和数据目录。详见 [管理 Session](manage-sessions.md)。

Ctrl-C 等待所拥有的命令清理。Session 日志和保留输出会跨关闭/重开保留。

存储与事件语义见 [文件布局](../reference/file-layout.md) 和 [事件](../reference/events.md)。
