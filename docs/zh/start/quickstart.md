# 快速开始

## 配置模型

在你的项目目录里打开终端。全新的 `KUKU_HOME` 先跑一次 `kuku init`（已有配置直接复用现有 `config.toml` 即可）。

默认生成的 `balanced` 档走 `anthropic` 连接，启动本地服务前在终端里设置 `ANTHROPIC_API_KEY`。如果要默认用 OpenAI，设置 `OPENAI_API_KEY`，把默认模型档切到 `openai` 连接并填入账号可用的模型 ID。详见[配置 kuku](configuration.md)。

## 在终端里开始第一个任务

```bash
kuku
```

输入「检查这个目录的文件，告诉我怎样运行测试」，按 Enter 发送。第一条消息会自动创建 Session，状态栏会显示当前 Workspace、Agent 和模型。

Agent 执行期间：

- **Enter**：发送补充输入，在下一个安全的模型请求边界生效。
- **Alt+Enter**：排队等到下一轮（Turn）再处理。
- 关掉终端不会打断任务，本地服务会在后台继续跑完已接下的工作。

更多日常用法见[运行与继续任务](../guides/run-a-task.md)。

## 回到同一个 Session

换一个终端进入同一工作目录，运行 `kuku`，输入 `/resume` 就能接上之前的 Session。如果你的构建内嵌了 Web UI，也可以用 `kuku web` 在浏览器里连同一个服务查看。[运行服务](../guides/run-the-server.md)

## 从脚本运行

```bash
kuku run --json "Inspect the files in this directory"
kuku list
kuku show SESSION_ID
```

把 `SESSION_ID` 换成 `kuku run` 打印出的 ID。需要临时指定线路时，在提示词前加上 `--connection openai --model YOUR_MODEL_ID`。[CLI 参考](../reference/cli.md)
