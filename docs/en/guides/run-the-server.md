# Run the Server

Use the server when another client needs HTTP access to kuku.

## Start the Server

```bash
kuku server --listen 127.0.0.1:17777
```

Optional flags:

- `--config <path>`
- `--password <token>`
- `--max-concurrent-runs <n>`

Exact arguments are in [CLI](../reference/cli.md).

## Remote Access Rule

If you bind to a non-loopback address, `--password` is required.

Example:

```bash
kuku server --listen 0.0.0.0:17777 --password <token>
```

## Check Health

```bash
curl http://127.0.0.1:17777/health
```

## Run a Task Over HTTP

Send `POST /runs` with a `prompt` and `workspace`. Add `connection` plus `model`, or `tier`, for explicit selection. `agent` chooses a stable address and defaults to `main`; another address has independent context. The response is NDJSON and its first event reports the Session identity and durable receipt.

The server uses the public Session SDK and trusted-local tool policy by default. HTTP subscribers only observe progress: disconnecting a client leaves execution running. Use `DELETE /runs/{id}` to cancel and await cleanup.

Use [Server API](../reference/server-api.md) for the exact request and response formats.

## Continue or Inspect Sessions

Read durable event pages with `after` and `limit`; use `next_after` for the next cursor. A deferred question/authorization requires its exact request identity through the response endpoint. Waiting does not consume a model execution slot.

The server also exposes:

- `GET /sessions`
- `GET /sessions/{id}/events`
- `GET /sessions/{id}/conversations`
- `DELETE /sessions/{id}`
- `POST /runs/{id}/responses`

## Related Pages

- [Server API](../reference/server-api.md)
- [Events](../reference/events.md)
