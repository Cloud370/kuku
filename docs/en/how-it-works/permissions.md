# Tool permissions

Before a tool runs, the Session binds the actual tool definition and final arguments to its call ID. Built-in guards and project policy can reject an action. For an eligible action, the host's `ToolAuthorizer` sees the Session, acting Agent, trusted input source, tool, arguments and workspace. The default CLI and HTTP server use `TrustedLocal` to allow eligible actions.

| Decision | Observable result |
|---|---|
| Allow | The runtime records dispatch before invoking the tool and records the result afterwards. |
| Deny | The call receives a blocked result. A host authorization error also prevents dispatch and is reported as an error. |
| Defer | The Agent waits with a durable request ID. The host answers that exact request for that Agent, including after reopening the Session. |

In an embedding host, use `SessionOptions.authorizer` to supply the caller's policy. In the CLI, use `kuku reply SESSION_ID REQUEST_ID --agent AGENT --allow` or `--deny`. A question from `ask_user` uses `--answer`. The runtime checks the prepared action and current tool definition before dispatching an approved deferred call. [Sessions](sessions.md#waiting-and-replies) · [Tool authorization](../../../crates/kuku/src/permission/runtime.rs)

For `run_command`, project policy matches a JSON string containing the canonical `cwd` and the `invocation`. The `invocation` carries its `mode` and either the literal `program` and `args` or the selected `shell` and its `command` text. This preserves argument boundaries in the authorization candidate.

The host authorizes `workspace_script` and each of its host actions. Deferring the outer call pauses before starting a worker. When the host defers a script action that can be prepared durably, the worker stops and releases its JavaScript state. The Session saves the prepared action and the facts already observed. Approval revalidates and can dispatch that action only. The model then continues from the returned facts. Derived output drafts belong to one invocation. A `Defer` decision on `create_output`, `append_output`, or `finish_output` returns `blocked` before dispatch. [Workspace scripts](../reference/tools.md#workspace-scripts)
