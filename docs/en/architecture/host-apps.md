# Host applications

The Session SDK owns Agent execution, the journal and retained output. A host opens the Session, supplies its tool authorization policy and presents results in the form its callers need.

| Entry | How it works today |
|---|---|
| `kuku` and `kuku run` | The full-screen terminal interface and script commands use the shared Rust HTTP client. The terminal owns drafts, focus and scroll position. |
| `kuku server` | A long-lived HTTP process owns open Sessions. Disconnecting an HTTP observer leaves execution running. The API supports durable commands, snapshots, history, observation and retained output. |
| Rust embedding | Applications can use the public `Session` SDK directly with their own configuration, authorization and observation strategy. |

The script commands live in `crates/kuku-cli`, the server in `crates/kuku-server`, the HTTP client in `crates/kuku-client` and the terminal interface in `crates/kuku-tui`. `apps/kuku` assembles the executable. [CLI reference](../reference/cli.md) · [Server API](../reference/server-api.md) · [SDK reference](../reference/sdk.md)

The managed local service has one identity per resolved data directory. Client windows provide the target workspace, Session and Agent, and each client maintains its own observation cursor. The browser client is an optional development target. [Interactive client plan](interactive-clients.md) · [Web client design](web-client.md)
