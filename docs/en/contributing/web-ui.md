# Web UI Development

Use this page when changing `apps/web/**`, the embedded Web assets, or the Web API contract.

## Start here

1. Read [Development](development.md) and [Testing](testing.md).
2. Install the Node version recorded in `apps/web/.nvmrc`.
3. From `apps/web`, run `npm ci`.
4. Start the UI with `npm run dev`. It connects to `kuku-server` at `127.0.0.1:17777`.

The compact command reference also lives in [apps/web/README.md](../../../apps/web/README.md).

## Local workflow

Run commands from `apps/web` unless noted otherwise.

```bash
npm ci
npm run typecheck
npm run lint
npm run test
npm run build
```

Use `npm run test:storybook` for component stories. Use `npm run test:e2e` for browser journeys
that start the embedded application binary. CI runs these journeys with one worker because each
test owns a real server process; local runs may opt into parallel workers when resources allow.

When a change affects the API contract, regenerate before testing:

```bash
npm run generate:api
git diff -- src/api/generated
```

Never edit `src/api/generated/**` directly.

## Embedded application

The production `kuku web` server serves assets embedded in `kuku-app`. Build the assets before
building that binary:

```bash
cd apps/web
npm run build
cd ../..
cargo build -p kuku-app --features embedded-web-assets
```

## Browser diagnostics

Browser E2E covers behavior, accessibility, and responsive bounds without committed image baselines.
Do not commit `apps/web/test-results/`, Playwright HTML reports, traces, screenshots, or videos.
They are generated failure diagnostics and are ignored by Git.

## CI ownership

| Workflow | Trigger | Responsibility |
| --- | --- | --- |
| `CI` | `main` changes outside pure Web UI paths and pull requests | Rust format, clippy, and cross-platform tests |
| `WebUI CI` | Relevant Web, embedded-app, contract, server, or workflow changes | Contract generation, Web static checks, and embedded Chromium journeys |
| `Release` | Version tags | Build, smoke-test, package, checksum, and publish release artifacts |

`WebUI CI` uploads browser reports only when a job fails. It is the authoritative automated Web
gate; do not add manual approval workflows for normal alpha changes.

## Agent checklist

1. Keep generated API files synchronized rather than hand editing them.
2. Add or update a focused unit test for behavior changes.
3. Run the relevant static checks and browser suite.
4. Keep generated browser diagnostics out of commits.
