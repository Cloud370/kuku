# Tool permissions

Before any tool runs, the Session binds the resolved tool definition and final arguments to its call ID. Built-in guards and project policy can block an action up front. For eligible actions, the host's `ToolAuthorizer` inspects the Session, acting Agent, trusted input source, tool, arguments, and workspace. Both the default CLI and HTTP server use `TrustedLocal`, which allows eligible local actions.

| Decision | Effect |
|---|---|
| `Allow` | Runtime records dispatch, invokes the tool, then commits the result. |
| `Deny` | Call receives a `blocked` result. A host authorizer error also blocks dispatch and surfaces as an error. |
| `Defer` | If supported by the action, the Agent yields with a durable `request_id` until the host replies (even across reopens). |

SDK embedders plug in custom policy via `SessionOptions.authorizer`. In the CLI, answer a deferred approval with `kuku reply SESSION_ID REQUEST_ID --agent AGENT --allow` or `--deny` (use `--answer` for `ask_user`). When approved, the runtime revalidates the prepared action and current tool definition before dispatching it. [Sessions](sessions.md#waiting-and-replies) · [Tool authorization source](../../../crates/kuku/src/permission/runtime.rs)

## `run_command`

Before authorization, the runtime resolves the canonical `cwd` and final `invocation` (including defaults). Project policy matches against a JSON string containing `cwd` and `invocation` (`mode` plus either literal `program`/`args` or `shell`/`command`). Authorization applies to that invocation as a whole; OS-level sandboxing, if required, is up to the host.

## `run_js` and background Jobs

The host authorizes both the outer `run_js` call and each host action the script invokes:

- Deferring the outer `run_js` pauses before starting a JS worker.
- Deferring a durably-preparable action inside a foreground script stops the worker, drops JS state, and saves the prepared action plus already-observed facts. Once approved, kuku revalidates and runs that action once, and the model continues from the returned facts.
- Derived output drafts (`create_output`, `append_output`, `finish_output`) require immediate authorization — returning `Defer` blocks them before dispatch.
- Detaching a running call into a [Session Job](sessions.md#background-jobs) keeps the already-authorized execution running, while subsequent script host actions still go through `ToolAuthorizer`. If a detached script hits an action requiring deferred approval, the Job stops before dispatching it and reports the facts observed so far.
