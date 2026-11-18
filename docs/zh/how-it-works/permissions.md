# 工具权限

工具执行前，Session 将实际工具定义和最终参数绑定到此次调用的 ID。内置限制和项目策略可以拒绝动作。对于可继续的动作，宿主的 `ToolAuthorizer` 接收 Session、执行的 Agent、可信输入来源、工具、参数及工作目录。默认 CLI 和 HTTP server 使用 `TrustedLocal` 放行符合这些条件的动作。

| 决定 | 可观察结果 |
|---|---|
| 允许 | 运行时先记录派发，再调用工具，随后记录结果。 |
| 拒绝 | 该调用收到被阻止的结果。宿主授权出错也会阻止派发，并返回错误。 |
| 延后 | Agent 进入等待，保留持久请求 ID。宿主按 Agent 和请求 ID 回答，重开 Session 后仍然有效。 |

嵌入方通过 `SessionOptions.authorizer` 提供自己的策略。CLI 使用 `kuku reply SESSION_ID REQUEST_ID --agent AGENT --allow` 或 `--deny` 回应授权，`ask_user` 的问题使用 `--answer`。延后授权获准后，运行时会重新核对已准备的动作和当前工具定义，再派发执行。[Session](sessions.md#等待与回复) · [工具授权接口](../../../crates/kuku/src/permission/runtime.rs)

对 `run_command`，项目策略匹配包含规范化 `cwd` 和 `invocation` 的 JSON 字符串。`invocation` 带有 `mode`，并提供字面的 `program` 和 `args`，或所选的 `shell` 和 `command` 文本。这样授权匹配时仍能区分各个参数。

宿主分别授权 `workspace_script` 及每个脚本宿主动作。延后外层工具调用的授权时，运行时在启动 worker 前进入等待。延后脚本内动作的授权时，worker 停止并释放 JavaScript 状态。Session 保存已准备的动作及已观察到的事实。批准后只重新核对并派发对应动作，模型随后根据返回的事实继续。见[工作区脚本](../reference/tools.md#工作区脚本)。
