# Tool permissions

A Session uses one permission mode for its Agents. Full access is the default. Choose a mode in the TUI with `F3` or `/permissions`. The TUI asks once after model setup in a workspace without a saved preference. Each Agent resolves relative paths from its own directory. Full access allows absolute paths outside that directory, including the system temporary directory.

| Mode | Workspace actions |
|---|---|
| Full access | Allow reads, writes, and commands subject to host policy. |
| Ask | Allow ordinary reads and ask before writes and commands. |
| Read-only | Allow ordinary reads and block writes and commands. |

The TUI keeps focus on the draft when an action is waiting. Press `F2` to review the exact action, Agent, directory, and reason. Use `Tab` to move between requests. `Y` allows once and `N` denies. `A` trusts the workspace and switches to Full access before approving an Ask-mode request. Host approval requests offer the one-time decisions. [Terminal commands](../reference/cli.md#terminal-interface)

## SDK and service

Set `SessionOptions.permission_mode` to select the starting mode and use `Session::set_permission_mode` for subsequent actions. A custom `SessionOptions.authorizer` receives the Session, Agent, trusted input source, exact tool definition, final arguments, and canonical working directory. It can allow, deny, or defer the action. Host denials apply in every mode. Permission mode and host policy are evaluated at the same boundary before the action runs.

Use `Session::pending_permissions` and `Session::reply_permission` to inspect and decide requests. From the CLI, use `kuku reply SESSION_ID REQUEST_ID --agent AGENT --allow` or `--deny`. Approvals bind to prepared actions. The runtime revalidates the target and applicable policy before dispatch. OS-level isolation is the host’s responsibility. [Server API](../reference/server-api.md)

## `run_js` and background Jobs

The host authorizes the outer `run_js` call and each host action. The workspace mode is applied to the individual actions. A deferred action pauses the script at its `await`. After approval the runtime executes the prepared action once and the same script continues. Denial returns a blocked result to the script. Other Agents can keep working while it waits.

The same flow applies to detached scripts. The approval panel shows the Job when one exists. A client can reconnect while the service is running and answer the waiting request. If the process restarts, the Session retains the prepared action and completed effects. Continuing an interrupted foreground script's Agent includes those facts in the next model request. The remaining script steps need a new invocation. [Background Jobs](sessions.md#background-jobs)
