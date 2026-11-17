# Host applications

The Session SDK owns Agent execution, the journal and retained output. A host opens the Session, supplies its tool authorization policy and presents results in the form its callers need.

| Entry | How it works today |
|---|---|
| `kuku` and `kuku run` | The CLI opens the SDK in the foreground. Bare `kuku` reads one line at a time, reopens the same Session between prompts and exits on `exit`, `quit` or EOF. |
| `kuku server` | A long-lived HTTP process owns open Sessions. Disconnecting an HTTP observer leaves execution running. Its API supports durable event pages, correlated replies and explicit cancellation. |
| `kuku web` | The unified binary starts the server and serves the browser app from bundled assets. Source builds need `embedded-web-assets` and a built `apps/web/dist`. |
| Rust embedding | Applications can use the public `Session` SDK directly with their own configuration, authorization and observation strategy. |

The CLI implementation lives in `crates/kuku-cli`, the server in `crates/kuku-server`, the unified executable in `apps/kuku` and the browser app in `apps/web`. [CLI reference](../reference/cli.md) · [Server API](../reference/server-api.md) · [SDK reference](../reference/sdk.md)

The planned Rust TUI, CLI and WebUI share a server-owned Session. Local server discovery, per-window focus and input timing are specified in the [interactive client plan](interactive-clients.md).
