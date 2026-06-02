# Run a Task

当 kuku 已经安装并完成配置后，就使用这个流程。

## Start a New Task

一次性运行：

```bash
kuku run "check this project"
```

如果要显式选择一个 tier：

```bash
kuku run --model strong "review this diff"
```

准确参数见 [CLI](../reference/cli.md)。

## Use Interactive Mode

如果要进行来回交互的工作，就启动交互式 CLI：

```bash
kuku
```

然后每一轮输入一个 Prompt。

## Continue Existing Work

恢复一个已知 Session：

```bash
kuku run --session <session-id> "continue"
```

或者继续最近一次 Session：

```bash
kuku run --continue "continue"
```

## Inspect Output

- `kuku show <session-id>` 用于查看最终回答
- `kuku events <session-id>` 用于查看持久化事件日志
- `kuku list` 用于查看当前工作区最近的 Session

## Adjust Output Mode

当结果要被其他 Tool 或脚本消费时，使用这些选项：

- `--json` 输出一行最终 JSON
- `--stream-json` 输出实时 JSON lines
- `--raw` 输出纯文本

## Roll Back a Turn

在交互模式中，使用 `/undo` 回退到更早的一轮。

关于 Session 存储和事件语义，参见 [File Layout](../reference/file-layout.md) 和 [Events](../reference/events.md)。
