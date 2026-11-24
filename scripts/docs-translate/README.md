# Docs Translate Tooling

Local and CI helpers for syncing public docs from `docs/en/**` into `docs/<locale>/**`.

## Files

- `.env.example`: copy to `.env.local` and fill provider or GitHub values as needed.
- `run-local-check.sh`: run the Docker-based local checker.
- `check-in-container.sh`: container entrypoint used by local and CI runs.
- `translate-page-and-open-pr.sh`: translate one page, then commit its mirror and publish a PR.
- `report-docs-en-changes.sh`: report updated and removed English docs paths for each push to `main`.

Do not commit `.env.local`.

## Setup

```bash
docker build -f scripts/docs-translate/Dockerfile -t kuku-agent-runner:local .
cp scripts/docs-translate/.env.example scripts/docs-translate/.env.local
```

Edit `.env.local` before model or PR runs:

- `CI_LLM_BASE_URL` for the Responses API base URL
- `CI_LLM_API_KEY` for the service key
- `GH_TOKEN` for PR helpers

The default cache root is `/tmp/kuku/docs-translate`. The runner defaults to the exact model ID `deepseek-flash` with high reasoning. Set `CI_LLM_MODEL` to select another Responses model that supports `high`. [Translation config](config.toml)

## Tooling Check

This builds the CLI and validates the translation config offline.

```bash
KUKU_CHECK_RUN=0 bash scripts/docs-translate/run-local-check.sh
```

## Model Smoke

This uses the built-in fixed prompt in `check-in-container.sh` and should print `READY`:

```bash
bash scripts/docs-translate/run-local-check.sh
```

Set `KUKU_CHECK_OUTPUT_MODE=json` for the structured result, including the final text in `response`. To save the JSON and print only the Session ID, use `session-id`. Use the same `KUKU_RUN_ID` to inspect the saved reply.

```bash
run_id="manual-smoke-001"
session_id="$(KUKU_RUN_ID="$run_id" KUKU_CHECK_OUTPUT_MODE=session-id bash scripts/docs-translate/run-local-check.sh)"
KUKU_RUN_ID="$run_id" KUKU_CHECK_RUN=0 KUKU_CHECK_SESSION_ID="$session_id" bash scripts/docs-translate/run-local-check.sh
```

## Open PR Helper

```bash
bash scripts/docs-translate/translate-page-and-open-pr.sh --locale zh --base main docs/en/start/install.md
```

The helper uses the GitHub repository configured as `origin`. For a new PR, set `--base` to the source branch or use the current branch. In Actions, a new PR aimed at the workflow ref uses the dispatch revision. For an update, pass the number of an open PR whose head is in `origin`. The runner uses the selected revision's docs in a scratch workspace and only publishes the requested target page. An unchanged page leaves the branch as it is.

The translation preset offers file and Skill tools. The model container uses a temporary credential file and gets a read-only binary. Git and GitHub steps run on the host. Actions caches the build directories.

If publication succeeded but PR creation failed, rerun the same Actions run to resume from the pushed translation commit.

## GitHub Workflows

- `.github/workflows/docs-translate.yml` validates this tooling and Docker image.
- `.github/workflows/docs-translate-en-changes.yml` reports changed `docs/en/**` pages on `main` pushes, with removed paths listed separately.
- `.github/workflows/docs-translate-open-pr.yml` manually runs a real translation and opens or updates a PR.

The open-PR workflow supplies the URL and key from the repository Actions secrets `CI_LLM_BASE_URL` and `CI_LLM_API_KEY`. Use the Responses API base URL. The repository Actions variable `CI_LLM_MODEL` can override the default model ID.

Dispatch the workflow on a ref that contains this version of the CLI and helpers.

```bash
gh workflow run .github/workflows/docs-translate-open-pr.yml \
  --ref main \
  -f source_path=docs/en/reference/config.md \
  -f locale=zh \
  -f base_branch=main
```
