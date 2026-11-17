# Interactive clients — server, Rust TUI and WebUI

This plan targets a first usable terminal session. Start work from the CLI, close the terminal, open a TUI window, find the same Agent, read its progress and send more work. A browser can join the same Session.

## Where the change starts

The core rewrite moved both CLI and server execution onto the public Session SDK. The server already demonstrates that dropping an HTTP observer lets the Agent finish its tool call and commit the result. The CLI still opens a Session itself. The server rejects fresh input for a running Agent, and its Agent listing reads an offline journal snapshot. The SDK currently admits queued input at Turn start. These are the specific paths this plan changes. [CLI execution](../../../crates/kuku-cli/src/commands/run.rs) · [Server run manager](../../../crates/kuku-server/src/run_manager.rs) · [Agent listing](../../../crates/kuku-server/src/routes/sessions.rs) · [SDK driver](../../../crates/kuku/src/session/runtime/driver.rs) · [Disconnect test](../../../crates/kuku-server/tests/session_execution.rs)

## Ownership and client contract

| Owner | Responsibility |
|---|---|
| Session SDK | A single writer per Session, one advancing chain per Agent, input admission, prepared model requests, tool settlement, waiting, cancellation and recovery. Embedders can call it directly. |
| Server | Own the open Sessions for product clients. Resolve workspace paths, model configuration, credentials and tool execution on the server machine. Expose durable commands, current Agent state and resumable observation over HTTP. Release idle runtime resources once owned work has settled. |
| Clients | Use one HTTP contract locally and remotely. CLI and TUI share a Rust client for submission, receipts, observation and reconnect. WebUI consumes the same contract. Selected Agent, scroll position, drafts and key bindings belong to each window. |

By default, reuse the user's local server. If none is available, start an independent loopback server on demand. Concurrent launches using the same service configuration converge on one instance, including when they start from different workspaces. For an explicitly selected remote server, connect to that address and report connection or authentication failures there.

Address commands by Session and Agent. A durable receipt acknowledges submission. Reusing a command ID for the same intent returns the same receipt, including after a lost connection. Reuse with a different target, text or timing is a conflict. Target interruption at the execution the user observed so a stale window cannot stop a later Turn. Answer questions and permissions with their pending request identity.

Attaching to an active Agent returns its live state and a committed cursor. A fresh client can read history and follow new commits across the snapshot boundary. Use bounded streaming for partial output and fill gaps from committed events. Listing, previewing or switching Agents sends no input and performs no inference. Closing the last client leaves active execution on the server. After a server restart, recover committed state and waits, and report an interrupted or unknown effect according to the SDK contract. [Session SDK](../reference/sdk.md) · [Current events endpoint](../../../crates/kuku-server/src/routes/events.rs)

## Terminal interaction

Use a single scrolling transcript, a fixed multiline editor showing its recipient, and a short status line. Collapse tool output to a summary with details available on demand. A new window starts blank. Create its Session on the first send. A command such as `/resume` opens the Session list with a history preview. Within a Session, one Agent selector changes the transcript and the recipient of the next explicit send. Two windows may select different Agents. [Pi interaction reference](https://github.com/earendil-works/pi/blob/e56d35ce5eceedfa16eaf13a5f01a1f7b7271253/packages/coding-agent/docs/usage.md)

| Action | Intended behavior |
|---|---|
| Send with `Enter` | An idle Agent starts work. A busy Agent accepts the input durably and admits it before the next unprepared model request in the current Turn, after the in-flight response and its dispatched tools settle. If the Turn finishes first, the input carries into the next Turn. |
| Follow up with `Alt+Enter` | Place the input in the next Turn, after the current Turn settles. Provide the same action in the command menu on terminals that intercept this key. |
| Interrupt with `Esc` | Close an open selector first. In the conversation, stop the selected Agent's observed execution and keep the draft in the editor. Show cleanup progress or failure. Only a subsequent `Enter` submits the draft. Closing the window detaches its observer. |

The SDK decides admission at each boundary. Preserve receipt order among inputs eligible at that boundary and keep a prepared Step identical across provider retries. A queued follow-up must allow a later steering message into the current Turn. Waiting for an answer or authorization remains visible and requires the matching response. [Prepared Steps](../../../crates/kuku/src/query/frozen.rs)

## Implementation order

1. **Land the verified SDK baseline on `main`.** Start one client branch from that commit and reuse the active checkout and its Cargo target. Retire superseded worktrees after checking commit coverage, running processes and evidence stored outside Git.
2. **Deliver one shared execution path.** Extend SDK admission at the Step and Turn boundaries. Adapt the server's existing Session owner, event reader and reply handling. Connect the CLI through the reusable Rust client. Two clients must be able to submit, detach, find the same Agent, steer it and observe one durable result. Replace superseded interfaces and their callers together. This contract can land as soon as its client path works. [Core Direction](core-direction.md)
3. **Deliver the terminal.** Use Ratatui for layout, Crossterm for terminal events and a multiline editor that supports soft wrap. A small screen prototype can progress while the server contract is implemented. Wire the normal interactive `kuku` entry to the TUI on a terminal. Explicit script commands retain machine-readable output. Verify skill invocation alongside local client commands. [Crossterm events](https://docs.rs/crossterm/latest/crossterm/event/struct.EventStream.html) · [Ratatui textarea](https://docs.rs/ratatui-textarea/latest/ratatui_textarea/)
4. **Connect the existing WebUI.** Update browser callers alongside any replaced HTTP routes. Use the same Session/Agent commands for reconnect, steering, follow-up and interruption. Verify that a browser can see work from the TUI and return control to it. Further browser layout work can follow daily use of the terminal. [Existing WebUI API](../../../apps/web/src/api)

## Testing during development

| Layer | What to exercise |
|---|---|
| Screen and input | With Ratatui's in-memory backend, render a normal and narrow terminal. Check recipient, draft, scroll, wide Chinese characters, wrapping and visible waiting or queued state. Drive key handling directly. [TestBackend](https://docs.rs/ratatui/latest/ratatui/backend/struct.TestBackend.html) |
| SDK and server | Use the real server, temporary workspace and controllable provider. Hold a tool call open, queue a follow-up, then send steering. Drop the first client after its receipt, reconnect and release the tool. Inspect recorded provider requests and Turn IDs. The next Step gets steering, the next Turn gets the follow-up, and a retried command appears once. Also race cancellation with a replacement Turn and check correlated replies. [Existing provider fixture](../../../crates/kuku-server/tests/common/mock_provider.rs) |
| Real terminal | Launch two clients against the same local configuration under a Unix PTY or native Windows ConPTY and verify they discover one server. Switch Agents, paste a draft, resize and interrupt with `Esc`. Check that the draft stays visible and its first receipt arrives only after `Enter`. Detach, reconnect, and check the durable result and restoration of terminal mode and cursor. [portable-pty](https://docs.rs/portable-pty/latest/portable_pty/) · [vt100 screen parser](https://docs.rs/vt100/latest/vt100/struct.Parser.html) |
| Native use | Try composing Chinese text, copying output and scrolling on Linux and Windows Terminal. Run the automated cases on macOS CI and report hands-on macOS coverage separately. Exercise one browser reconnect against the same server. [Testing](../contributing/testing.md) |
