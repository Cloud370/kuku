# Run the server

## Managed local service

Running `kuku` or `kuku run` automatically finds or starts a loopback service for your resolved `KUKU_HOME` data directory. Clients in different project directories share that service, each passing its own workspace path, and closing a terminal leaves accepted work running.

- `kuku server status` — check the running build, protocol version, and active config path
- `kuku server restart` — reload a new binary or updated environment variables (wait for active work to settle if busy)
- `kuku server restart --config PATH` — switch the managed service to another config file

See [Upgrade kuku](upgrade-kuku.md) for version handoff behavior.

## Open the Web UI

With a [build that embeds the Web assets](../reference/cli.md#kuku-server-and-kuku-web), run `kuku web` from your workspace (optionally with `--session SESSION_ID --agent ADDRESS`), or type `/web` inside the TUI for the selected Agent. Browser and terminal clients can observe the same Session and reply to the same pending requests.

A managed local launch signs in with a single-use ticket. Pointing `kuku web` at an explicit server opens a sign-in page where you enter that server's access token. [CLI reference](../reference/cli.md#kuku-server-and-kuku-web) · [HTTP authentication](../reference/server-api.md#authentication-and-versions)

Browser shortcuts match the TUI: **Enter** sends or steers, **Shift+Enter** inserts a newline, and **Alt+Enter** queues for the next Turn. Use the Agent selector to choose the recipient, **Stop** to interrupt, **Continue** to resume, and **Withdraw** to drop a queued input. If a receipt is lost, the page lets you re-check or resend the original command. [Sessions](../how-it-works/sessions.md)

## Start an explicit server

On the server host, initialize `KUKU_HOME`, configure provider keys, set `KUKU_SERVER_PASSWORD` to a secret token, and keep the server running:

```bash
kuku server --listen 127.0.0.1:17777
```

Connect from another terminal with `KUKU_TOKEN` set to the same token, passing an absolute workspace path on the server host:

```bash
kuku --remote http://127.0.0.1:17777 --workspace /code/my-project run "Summarize this directory"
```

Passing `--remote` never silently falls back to a local service if the connection fails. Across machines, put the server behind an authenticated HTTPS proxy or secure tunnel. Provider credentials and tool permissions always come from the server's configuration. [Connection flags](../reference/cli.md#connection-and-workspace-options)

## Health check and HTTP API

```bash
curl http://127.0.0.1:17777/health
```

`/health` returns the build and protocol versions without auth. Session commands and event streams require `Authorization: Bearer TOKEN`. Reuse the same Session and command IDs to recover from a lost receipt, and pass your last committed cursor to resume streaming. [Server API](../reference/server-api.md)
