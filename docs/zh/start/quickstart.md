# Quickstart

## 1. 启动 kuku

```bash
kuku
```

打开终端输出的 credential URL。首次运行时，Web UI 会引导你配置 provider 和 workspace。

## 2. 配置 Provider

在 Web 设置中填写 provider API key。若只使用终端，默认配置也支持 `ANTHROPIC_API_KEY` 或 `OPENAI_API_KEY`：

```bash
export ANTHROPIC_API_KEY="..."
```

参见 [Environment Variables](../reference/environment-variables.md) 和 [Config](../reference/config.md)。

## 3. 运行第一个任务

在 Web UI 中创建并运行任务。

如需使用终端，请在完成配置后运行 `kuku run say hello`。

## 4. Inspect the Result

一些常用的后续命令：

```bash
kuku list
kuku show <session-id>
kuku events <session-id>
```

完整命令面请参见 [CLI](../reference/cli.md)。

## Next

- 如果是常规任务流程，前往 [Run a Task](../guides/run-a-task.md)。
- 如果要看配置细节，前往 [Configuration](configuration.md)。
