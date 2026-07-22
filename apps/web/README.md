# kuku WebUI

Frontend SPA for [kuku](https://github.com/Cloud370/kuku) — a file-native AI agent SDK. Talks to `kuku-server` via HTTP + NDJSON streaming.

## Quick start

```bash
npm ci
npm run dev
```

Requires `kuku-server` running on `127.0.0.1:17777`.

For the full contributor workflow, embedded-binary setup, and browser-diagnostic rules, see
[Web UI development](../../docs/en/contributing/web-ui.md).

## Commands

| Command | Description |
|---------|-------------|
| `npm run dev` | Start Vite dev server |
| `npm run typecheck` | Run TypeScript type checking |
| `npm run lint` | Run ESLint |
| `npm run test` | Run Vitest tests |
| `npm run test:storybook` | Run Storybook component tests |
| `npm run build` | Production build to `dist/` |
| `npm run storybook` | Start Storybook component explorer |
| `npm run test:e2e` | Run embedded-browser journeys |

## Generated API and test diagnostics

- Do not edit `src/api/generated/**` by hand. Run `npm run generate:api` after an API-contract change.
- `test-results/`, Playwright reports, traces, and videos are failure diagnostics. They are ignored
  and must not be committed.

## Tech stack

React 19 · TypeScript strict · Vite 8 · Tailwind CSS v4 · TanStack Query v5 · Zustand · Storybook 10 · Vitest
