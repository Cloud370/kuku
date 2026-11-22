# 工具权限

工具执行前，Session 会把生效的工具定义和最终参数绑定到这次调用的 ID 上。内置保护和项目策略可以直接拦截违规动作；通过初筛的动作交给宿主的 `ToolAuthorizer`，传入 Session、执行 Agent、可信输入来源、工具、参数和工作区。默认 CLI 和 HTTP 服务都使用 `TrustedLocal`，放行符合条件的本地动作。

| 决定 | 效果 |
|---|---|
| `Allow` | 先记录派发，再调用工具，最后落盘结果。 |
| `Deny` | 该调用收到 `blocked` 结果；宿主授权本身出错同样阻止派发并报错。 |
| `Defer` | 若动作支持延后审批，Agent 带持久 `request_id` 暂停等待，直到宿主回复（重开 Session 依然有效）。 |

嵌入 SDK 时通过 `SessionOptions.authorizer` 注入自定义策略。CLI 用 `kuku reply SESSION_ID REQUEST_ID --agent AGENT --allow` 或 `--deny` 回复授权（`ask_user` 用 `--answer`）。批准后，运行时会重新核对已准备动作和当前工具定义再派发。[Session 等待与回复](sessions.md#等待与回复) · [工具授权源码](../../../crates/kuku/src/permission/runtime.rs)

## `run_command`

授权前，运行时会先解析规范化的 `cwd` 和最终 `invocation`（含默认值）。项目策略匹配的是包含 `cwd` 与 `invocation` 的 JSON 字符串（`invocation` 带 `mode`，以及字面 `program`/`args` 或选中的 `shell`/`command`）。授权覆盖整次调用；如果需要操作系统级沙箱隔离，由宿主负责。

## `run_js` 与后台 Job

宿主会分别审批外层 `run_js` 调用以及脚本内发起的每一个宿主动作：

- 延后外层 `run_js` 会在启动 JS worker 之前暂停。
- 延后前台脚本里某个可持久准备的动作时，worker 会停止并释放 JS 状态，Session 保存已准备好的动作和已观察到的事实；批准后重新核对并派发一次，模型接着这些事实继续推进。
- 派生输出草稿（`create_output`、`append_output`、`finish_output`）要求即时授权，返回 `Defer` 会在派发前直接 `blocked`。
- 运行中的调用转成[后台 Job](sessions.md#后台-job) 后，已获批的执行继续跑，脚本内后续宿主动作依然要过授权；如果已分离的脚本遇到需要延后审批的动作，Job 会在派发前停止并汇报已观察到的事实。
