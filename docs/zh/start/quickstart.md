# 快速开始

## 配置模型

在希望 Agent 工作的目录打开终端。如果是新的 kuku 数据目录，先运行一次 `kuku init`。已有安装可以继续使用现有的 `config.toml`。

生成的 `balanced` 档使用 `anthropic` 连接。启动本地服务前，在终端环境中设置 `ANTHROPIC_API_KEY`。如果想默认使用 OpenAI，设置 `OPENAI_API_KEY`，将默认模型档的连接设为 `openai`，并填入账号可用的模型 ID。[配置指南](configuration.md)

## 在终端开启任务

```bash
kuku
```

输入「检查这个目录的文件，告诉我怎样运行测试」，按 Enter 发送。首次发送会创建 Session。状态栏显示工作区、Agent 和模型。查看回复和工具结果后，可以继续提问。

Agent 工作期间按 Enter 发送，会在下一个安全的模型请求边界接纳输入。按 Alt+Enter 可以把输入排到下一轮。关闭终端客户端后，服务会继续已接下的工作。[执行任务](../guides/run-a-task.md)

## 回到同一个 Session

在另一个终端进入同一工作区，启动 `kuku`，用 `/resume` 选择 Session。使用内嵌 Web 资源的构建时，也可以在浏览器连接同一个服务并打开该 Session。[运行服务](../guides/run-the-server.md)

## 从脚本运行

```bash
kuku run --json "Inspect the files in this directory"
kuku list
kuku show SESSION_ID
```

将 `SESSION_ID` 换成打印出的标识。要选择精确的模型连接，可以在提示词前加上 `--connection openai --model YOUR_MODEL_ID`。[CLI 参考](../reference/cli.md)
