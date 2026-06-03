# Docs Translate Tooling

Local and CI helpers for syncing public docs from `docs/en/**` into `docs/<locale>/**`.

## Files

- `.env.example`: copy to `.env.local` and fill provider or GitHub values as needed.
- `run-local-check.sh`: run the Docker-based local checker.
- `check-in-container.sh`: container entrypoint used by local and CI runs.
- `translate-page-and-open-pr.sh`: ask the agent to translate one page and create or update a PR.
- `report-docs-en-changes.sh`: deterministic GitHub summary for `main` changes under `docs/en/**`.

Do not commit `.env.local`.

## Setup

```bash
docker build -f scripts/docs-translate/Dockerfile -t kuku-agent-runner:local .
cp scripts/docs-translate/.env.example scripts/docs-translate/.env.local
```

Edit `.env.local` before model or PR runs:

- `KUKU_ANTHROPIC_BASE_URL`
- `KUKU_ANTHROPIC_API_KEY`
- `GH_TOKEN` for PR helpers

The default cache root is `/tmp/kuku/docs-translate`.

## Tooling Check

This validates the container path and local build without calling a model:

```bash
KUKU_CHECK_RUN=0 bash scripts/docs-translate/run-local-check.sh
```

## Model Smoke

This uses the built-in fixed prompt in `check-in-container.sh` and should print `READY`:

```bash
bash scripts/docs-translate/run-local-check.sh
```

JSON mode returns run metadata. Use the same `KUKU_RUN_ID` when reading the final text with `kuku show`:

```bash
run_id="manual-smoke-001"
session_id="$(KUKU_RUN_ID="$run_id" KUKU_CHECK_OUTPUT_MODE=session-id bash scripts/docs-translate/run-local-check.sh)"
KUKU_RUN_ID="$run_id" KUKU_CHECK_RUN=0 KUKU_CHECK_SESSION_ID="$session_id" bash scripts/docs-translate/run-local-check.sh
```

`kuku run --json` currently returns metadata but not the final assistant text, so helpers use `session_id` plus `kuku show`. See issue #48.

## Open PR Helper

```bash
bash scripts/docs-translate/translate-page-and-open-pr.sh --locale zh --pr 46 docs/en/start/install.md
```

The helper passes structured context only: source path, target locale, target path, current branch, base branch, target existence, and optional PR reference.
It ignores `KUKU_CHECK_PROMPT` and always builds its own structured prompt.

## GitHub Workflows

- `.github/workflows/docs-translate.yml` validates this tooling and Docker image.
- `.github/workflows/docs-translate-en-changes.yml` reports changed `docs/en/**` pages on `main` pushes.
- `.github/workflows/docs-translate-open-pr.yml` manually runs a real translation and opens or updates a PR.

Before #46 is merged, run the open-PR workflow against `feat/docs-translate-tooling` so the workflow branch contains these helper scripts. After #46 is merged, run it from `main`.

Example GitHub Actions dispatch from this branch:

```bash
gh workflow run .github/workflows/docs-translate-open-pr.yml \
  --ref feat/docs-translate-tooling \
  -f source_path=docs/en/reference/config.md \
  -f locale=zh \
  -f base_branch=feat/docs-translate-tooling
```
