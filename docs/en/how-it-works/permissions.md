# Tool permissions

Before a tool runs, the Session binds the actual tool definition and final arguments to its call ID. Built-in guards and project policy can reject an action. For an eligible action, the host's `ToolAuthorizer` sees the Session, acting Agent, trusted input source, tool, arguments and workspace. The default CLI and HTTP server use `TrustedLocal` to allow eligible actions.

| Decision | Observable result |
|---|---|
| Allow | The runtime records dispatch before invoking the tool and records the result afterwards. |
| Deny | The call receives a blocked result. A host authorization error also prevents dispatch and is reported as an error. |
| Defer | The Agent waits with a durable request ID. The host answers that exact request for that Agent, including after reopening the Session. |

In an embedding host, use `SessionOptions.authorizer` to supply the caller's policy. In the CLI, use `kuku reply SESSION_ID REQUEST_ID --agent AGENT --allow` or `--deny`. A question from `ask_user` uses `--answer`. The runtime checks the prepared action and current tool definition before dispatching an approved deferred call. [Sessions](sessions.md#waiting-and-replies) · [Tool authorization](../../../crates/kuku/src/permission/runtime.rs)
