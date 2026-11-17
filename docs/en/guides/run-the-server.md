# Run the Server

Start an HTTP server when you want a task to keep running after you disconnect its client. Keep the server process running. The foreground CLI currently opens a separate SDK Session.

## Start the Server

Initialize kuku and set the API key for your selected connection. Start the server from the project directory and leave this terminal open. [Quickstart](../start/quickstart.md)

```bash
kuku server --listen 127.0.0.1:17777
```

Optional flags:

- `--config <path>`
- `--password <token>`
- `--max-concurrent-runs <n>`

By default the server reads `~/.kuku/config.toml`. If `kuku init` wrote the config under a custom `KUKU_HOME`, pass that file with `--config`. Session storage still follows `KUKU_HOME`. The workspace in an HTTP request is a path on the server machine. [CLI reference](../reference/cli.md)

## Remote Access Rule

For a non-loopback listener, set the shell variable `server_password` to your chosen token. Pass the same value in the remote client's `Authorization: Bearer` header.

```bash
kuku server --listen 0.0.0.0:17777 --password "${server_password:?Set server_password first}"
```

## Check Health

```bash
curl http://127.0.0.1:17777/health
```

## Run a Task Over HTTP

From a second terminal, send a prompt to the local server. Here `.` selects the directory where the server was started.

```bash
curl -N -H 'Content-Type: application/json' \
  -d '{"prompt":"Summarize this directory","workspace":"."}' \
  http://127.0.0.1:17777/runs
```

The first NDJSON line reports the Session ID and input receipt. Add `connection` plus `model`, or `tier`, to select a route. The `agent` field selects a stable address and defaults to `main`. Another address has independent context.

The server uses the public Session SDK and trusted-local tool policy by default. HTTP subscribers only observe progress: disconnecting a client leaves execution running. Use `DELETE /runs/{id}` to cancel and await cleanup.

Use [Server API](../reference/server-api.md) for the exact request and response formats.

## Continue or Inspect Sessions

Read durable event pages with `after` and `limit`; use `next_after` for the next cursor. A deferred question/authorization requires its exact request identity through the response endpoint. Waiting does not consume a model execution slot. After restarting the server, submit the matching reply through `POST /sessions/{id}/responses` with its original workspace, Agent, and request identity.

The server also exposes:

- `GET /sessions`
- `GET /sessions/{id}/events`
- `GET /sessions/{id}/conversations`
- `DELETE /sessions/{id}`
- `POST /runs/{id}/responses`
- `POST /sessions/{id}/responses`

## Related Pages

- [Server API](../reference/server-api.md)
- [Events](../reference/events.md)
