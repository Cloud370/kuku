# 为什么使用 kuku

kuku 是一个 Rust SDK，用于需要多个模型请求、工具或 Agent 协作完成的工作。你可以直接嵌入应用，也可以通过终端、脚本或浏览器客户端连接它的 HTTP 服务。

## 检查结果

每个 Session 都保存输入、已准备的模型请求、工具动作和最终结果。Agent 编辑文件或执行命令后，可以把记录的动作和工作目录中的实际结果对照。通过 `kuku show SESSION_ID` 查看结果，通过 `kuku events SESSION_ID` 读取底层事实。[管理 Session](../guides/manage-sessions.md)

## 继续任务

Session 可以重开。具名 Agent 保留自己的上下文、待决请求和工具输出。另一个 Agent 可以在该 Session 中独立处理相同的项目文件。通过 HTTP server 发起的任务在客户端断开后仍会继续。[文件原生执行](file-native-model.md) · [运行服务](../guides/run-the-server.md)
