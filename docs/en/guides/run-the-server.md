# Run the Server

## Managed local service

Launch `kuku` from your project directory or submit a script task with `kuku run`. The client finds or starts a loopback service for your resolved kuku data directory. Clients in different projects share the service and pass their own resolved workspaces. You can close a terminal client while the service finishes work.

Run `kuku server status` to check the running build, protocol and effective configuration. Use `kuku server restart` to load a new executable or environment. If the service reports active work or cleanup, let that work settle and retry. To switch configuration paths, pass `--config PATH` to the restart. [Upgrade kuku](upgrade-kuku.md)

## Continue in the browser

With a release bundle or a source build that embeds the Web assets, run `kuku web` from the workspace. Reopen a retained Session in the browser or pass `--session SESSION_ID --agent ADDRESS` when opening it. You can also use `/web` on the selected Agent in the terminal interface. The browser and terminal can observe the same work and answer the same pending requests.

A managed local launch authenticates with a single-use ticket. An explicitly selected server with bundled Web assets opens a sign-in page, where you enter the server access token. [CLI reference](../reference/cli.md#kuku-server-and-kuku-web) · [HTTP authentication](../reference/server-api.md#authentication-and-versions)

In the browser, Enter sends or steers, Shift+Enter inserts a newline, and Alt+Enter queues for the next Turn. Use the Agent selector to set the recipient. Use Stop to interrupt the selected execution, Continue to resume, and Withdraw to remove a queued input. After a lost receipt, the page can check it again or resend the original command. [Sessions](../how-it-works/sessions.md)

## Start an explicit server

Initialize the server's kuku data directory and set the provider credentials on that machine. Run this command in a shell with `KUKU_SERVER_PASSWORD` set to a token you choose. Keep the server process running while clients use it.

```bash
kuku server --listen 127.0.0.1:17777
```

Connect from another terminal with `KUKU_TOKEN` set to the same token. Replace the workspace with an absolute path on the server machine.

```bash
kuku --remote http://127.0.0.1:17777 --workspace /code/my-project run "Summarize this directory"
```

An explicit address stays selected even if the connection fails. For a server on another machine, use an authenticated HTTPS endpoint or a secure tunnel. The server's configuration determines provider access and tool permissions. [Connection flags](../reference/cli.md#connection-and-workspace-flags)

## Check health and call the API

```bash
curl http://127.0.0.1:17777/health
```

This reports the build and protocol versions. For Session commands and observations, send `Authorization: Bearer TOKEN`. Use the same Session and command IDs to recover from a lost receipt, and keep a committed cursor to resume observations. [Server API](../reference/server-api.md)
