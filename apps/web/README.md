# kuku WebUI

The browser interface uses the same authenticated HTTP and NDJSON Session protocol as the terminal. The release binary serves the application from the same origin as the API. Run `kuku web` to open the managed service, or use `kuku web --session ID --agent ADDRESS` to open a particular conversation. The workspace defaults to the launch directory.

In a local browser handoff, a temporary ticket authorizes the tab and is removed from the address bar. Refresh the same tab to reuse that connection. To open the WebUI served by an explicit remote server, use its HTTPS address and enter its server access token. That token is held in page memory.

## Development

Start an explicit local service with `KUKU_SERVER_PASSWORD` set and run `kuku server --listen 127.0.0.1:17777`. Use Node 24 to run these commands in `apps/web`.

```bash
npm ci
npm run dev
```

Enter the explicit server access token on the development page. Vite proxies API requests to `http://127.0.0.1:17777`. Set `KUKU_WEB_DEV_SERVER` before launching Vite to select another development server.

| Command | Description |
| --- | --- |
| `npm run typecheck` | Check TypeScript |
| `npm run lint` | Run ESLint |
| `npm run test` | Run Vitest, including shared protocol fixtures |
| `npm run build` | Build the production assets into `dist/` |
| `npm run storybook` | Open the component explorer |

After building the production assets, `cargo build -p kuku-app --features embedded-web-assets` embeds them in the unified binary. The installed browser handoff can be checked with `python3 scripts/acceptance/run.py interactive --browser` from the repository root.
