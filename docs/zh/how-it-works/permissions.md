# 工具权限

工具执行前，Session 将实际工具定义和最终参数绑定到此次调用的 ID。内置限制和项目策略可以拒绝动作。对于可继续的动作，宿主的 `ToolAuthorizer` 接收 Session、执行的 Agent、可信输入来源、工具、参数及工作目录。默认 CLI 和 HTTP server 使用 `TrustedLocal` 放行符合这些条件的动作。

| 决定 | 可观察结果 |
|---|---|
| 允许 | 运行时先记录派发，再调用工具，随后记录结果。 |
| 拒绝 | 该调用收到被阻止的结果。宿主授权出错也会阻止派发，并返回错误。 |
| 延后 | Agent 进入等待，保留持久请求 ID。宿主按 Agent 和请求 ID 回答，重开 Session 后仍然有效。 |

嵌入方通过 `SessionOptions.authorizer` 提供自己的策略。CLI 使用 `kuku reply SESSION_ID REQUEST_ID --agent AGENT --allow` 或 `--deny` 回应授权，`ask_user` 的问题使用 `--answer`。延后授权获准后，运行时会重新核对已准备的动作和当前工具定义，再派发执行。[Session](sessions.md#等待与回复) · [工具授权接口](../../../crates/kuku/src/permission/runtime.rs)
