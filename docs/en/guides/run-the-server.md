# Run the server

## Managed local service

Running `kuku` or `kuku run` automatically finds or starts a loopback service for your resolved `KUKU_HOME` data directory. Clients in different project directories share that service, each passing its own workspace path, and closing a terminal leaves accepted work running.

- `kuku server status` — check the running build, protocol version, and active config path
- `kuku server restart` — reload a new binary or updated environment variables (wait for active work to settle if busy)
- `kuku server restart --config PATH` — switch the managed service to another config file

See [Upgrade kuku](upgrade-kuku.md) for version handoff behavior.

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
