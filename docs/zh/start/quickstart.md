# 快速开始

## 发送第一条消息

[安装 kuku](install.md)，在项目目录里打开终端，运行：

```bash
kuku
```

全新的数据目录会进入配置流程。选择 Provider 预设或添加自定义连接，用自己决定的方式提供凭据，再选择一个精确的模型 ID。选 **Save and continue** 将它保存为新 Session 的默认值。设置页按 `F1` 可打开与构建版本对应的配置指南。[配置 kuku](configuration.md)

在输入框里输入「检查这个目录的文件，告诉我怎样运行测试」，按 Enter 发送。第一条消息会创建 Session，状态栏会显示当前工作区、Agent 和模型。

Agent 执行期间，按 **Enter** 发送补充输入，在下一个安全的模型请求边界生效。按 **Alt+Enter** 将消息排到下一轮。关掉终端后，本地服务会继续处理已接下的工作。[终端操作](../reference/cli.md#终端界面)

## 询问用法

配好模型后，可以直接问「怎么恢复上一次会话？」。Agent 可以查阅运行中的服务随附的文档。按 `F1` 或输入 `/help` 可再次找到提示。也可以从 `/` 菜单选择 `kuku-docs`，在 `/skill:kuku-docs` 前缀后输入问题。[使用 Skill](../guides/use-skills.md#询问-kuku-用法)

## 回到同一个 Session

换一个终端进入同一工作区，运行 `kuku`，用 `/resume` 重开之前的 Session。[管理 Session](../guides/manage-sessions.md)

## 从脚本运行

在终端或脚本里使用 `kuku run`。[无头任务指南](../guides/run-a-task.md)介绍如何在 shell 里配置、从文件或标准输入提交任务，再读取结果。
