# Quickstart

## 1. Initialize kuku

```bash
kuku init
```

这会创建默认运行时目录，并生成一个起步用的 `config.toml`。

## 2. Set a Provider API Key

默认配置期望以下环境变量中的一个：

```bash
export ANTHROPIC_API_KEY="..."
```

或者：

```bash
export OPENAI_API_KEY="..."
```

参见 [Environment Variables](../reference/environment-variables.md) 和 [Config](../reference/config.md)。

## 3. Run a First Task

```bash
kuku run say hello
```

或者启动交互模式：

```bash
kuku
```

不带 subcommand 时，会在当前工作区启动一个交互式 Session。

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
