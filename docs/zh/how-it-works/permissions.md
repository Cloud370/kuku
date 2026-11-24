# 工具权限

同一 Session 的 Agent 共用一个权限模式，默认全开放。TUI 可用 `F3` 或 `/permissions` 选择。没有已保存权限偏好的工作区会在模型就绪后询问一次。每个 Agent 从自己的目录解析相对路径。全开放允许通过绝对路径访问目录外的文件，包括系统临时目录。

| 模式 | 工作区操作 |
|---|---|
| 全开放 | 在宿主策略允许时读写文件和执行命令。 |
| 询问 | 放行普通读取，写入和执行命令前询问。 |
| 只读 | 放行普通读取，拦截写入和命令。 |

有待审批操作时，TUI 保留输入焦点。按 `F2` 查看实际操作、Agent、目录和询问原因，按 `Tab` 切换请求。`Y` 允许一次，`N` 拒绝。对询问模式产生的请求，`A` 信任此工作区，切换到全开放后批准当前操作。宿主发起的审批可选择允许一次或拒绝。[终端命令](../reference/cli.md#终端界面)

## SDK 和服务

用 `SessionOptions.permission_mode` 设定初始模式，用 `Session::set_permission_mode` 更新后续操作的模式。自定义 `SessionOptions.authorizer` 可检查 Session、Agent、可信输入来源、实际工具定义、最终参数和规范化工作目录，并决定允许、拒绝或等待审批。宿主拒绝在各种模式下都生效。工作区模式和宿主策略在派发前的同一个入口判定。

用 `Session::pending_permissions` 查看请求，用 `Session::reply_permission` 回复。CLI 可用 `kuku reply SESSION_ID REQUEST_ID --agent AGENT --allow` 或 `--deny`。批准绑定到已准备的动作，派发前重新校验目标和生效策略。操作系统级隔离由宿主负责。[服务端 API](../reference/server-api.md)

## `run_js` 与后台 Job

宿主分别检查外层 `run_js` 和每个宿主动作。工作区模式作用于脚本内的每个动作。需要批准时，脚本暂停在对应的 `await`。批准后运行时执行已准备的动作一次，原脚本继续。拒绝时结果以 `blocked` 返回，其他 Agent 在等待期间可以继续工作。

后台脚本同样支持这一流程，审批面板会标出所属 Job。服务进程仍在运行时，客户端重连后可以回复。如果进程重启，Session 保留已准备的动作和已完成的效果。继续前台脚本所在的 Agent 时，下一次模型请求会带上这些事实。剩余脚本步骤需要重新发起。[后台 Job](sessions.md#后台-job)
