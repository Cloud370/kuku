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

Send `POST /runs` with a `prompt` and `workspace`. The response is NDJSON.

Use [Server API](../reference/server-api.md) for the exact request and response formats.

## Continue or Inspect Sessions

The server also exposes:

- `GET /sessions`
- `GET /sessions/{id}/events`
- `DELETE /sessions/{id}`
- `POST /runs/{id}/responses`

## Related Pages

- [Server API](../reference/server-api.md)
- [Events](../reference/events.md)
