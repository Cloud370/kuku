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

需要连续对话时启动交互 CLI：

```bash
kuku
```

逐个 Turn 输入 Prompt。交互宿主在输入之间重开同一 Session。

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
