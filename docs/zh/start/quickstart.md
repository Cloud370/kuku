# 快速开始

## 初始化 CLI

进入希望 Agent 工作的目录。在新的 kuku 数据目录中，只需运行一次初始化命令。

```bash
kuku init
```

已有安装可以继续使用现有的 `config.toml`。CLI 从 `KUKU_HOME` 读取这个文件，默认目录是 `~/.kuku`。

## 运行第一个任务

生成的 `balanced` 档使用 `anthropic` 连接。在当前终端设置 `ANTHROPIC_API_KEY` 后运行下面的命令。

```bash
kuku run "Inspect the files in this directory"
```

使用 OpenAI 时，在当前终端设置 `OPENAI_API_KEY`，将 `YOUR_MODEL_ID` 换成账号可用的模型名称，并显式选择已配置的 Responses 连接。

```bash
kuku run --connection openai --model YOUR_MODEL_ID "Inspect the files in this directory"
```

要长期使用这条路线，可以把配置中的默认档指向该连接和模型。[配置指南](configuration.md)

## 继续或查看 Session

```bash
kuku list
kuku run --session SESSION_ID "Summarize what you found"
kuku show SESSION_ID
```

将 `SESSION_ID` 换成打印出的标识，在同一工作目录运行。不带参数的 `kuku` 打开逐行输入界面，后续输入沿用同一个 Session，两轮之间可以输入 `exit` 或 `quit` 退出。需要关闭客户端后任务仍继续时，从 HTTP server 发起任务。[运行任务](../guides/run-a-task.md) · [运行服务](../guides/run-the-server.md)
