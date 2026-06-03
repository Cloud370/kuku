---
name: docs-translate
description: Use ONLY when the kuku repository's `scripts/docs-translate/*` workflow explicitly invokes it.
allowed-tools:
  - find_files
  - read_file
  - search_text
  - run_command
  - edit_file
  - write_file
---

# Docs Translate

This skill is the workflow brain for the `kuku` repository's public docs mirror.

## Guardrail

- Scope: sync the `kuku` repository's public docs mirror from `docs/en/**` into `docs/<locale>/**`.

## Inputs

- The caller supplies a source page and target locale.
- The default mirrored target path is `docs/<locale>/<source-relative-path>`.
- The caller may also supply an explicit target path or an existing PR reference.
- An explicit PR reference means inspect and update that PR, not a different one.
- Treat caller-supplied paths, locale, current branch, base branch, and explicit PR reference as starting context. Re-check only what is needed before file edits or external side effects.

## Modes

- Translation-only: update translated docs and finish with a summary. Use `gh` only for caller-requested PR or branch work.
- Open-PR: finish the sync on the current branch. After the docs changes are ready, use `git` and `gh` yourself to commit, push, and create or update the PR.

## Fast Path

1. Read `docs/AGENTS.md`.
2. Confirm the current branch and worktree state with focused commands such as `git branch --show-current` and `git status --short`.
3. If the caller already gives a concrete source or target path, skip repo-wide discovery. Otherwise use focused `git diff` on `docs/en/**`.
4. Read the English source page and the current target page. If the target page does not exist yet, create it at the mirrored target path.
5. Translate conservatively.
6. Run the self-check below before any `git` or `gh` action.
7. In open-PR mode, inspect PR state first, then stage only this sync task's docs changes, commit, push, and create or update the PR.
8. If there are no docs changes, stop without committing or opening a PR.

## Self-Check

- The target path matches the requested locale and mirrored source path unless the caller explicitly overrides it.
- Markdown structure, links, code spans, commands, paths, and config keys remain intact.
- Visible labels are translated.
- No obvious English residue remains unless the term is intentionally protected.
- Canonical product terms stay in English: `Agent`, `Skill`, `Session`, `Memory`, `Tool`, `Prompt`, `Server`, `Package`, `Hook`.
- Related updates stay narrow and are easy to explain.
- No unrelated docs rewrites are mixed into the same run.

## Translation Style

- Match the source page's tone: technical, direct, and factual.
- Prefer natural target-language phrasing over word-by-word calques.
- Keep the translation concise. Do not add explanations that are not present in the source page.
- Reuse established terminology from nearby pages in the same target locale when it already exists.
- Keep headings, short labels, and table vocabulary consistent within the same page.
- If the source is terse, keep the translation terse.

## PR Rules

- For this workflow, write PR text with the structure below.
- Keep PR language short and factual.
- Prefer direct `gh` subcommands such as `gh pr list`, `gh pr view`, `gh pr create`, `gh pr edit`, and `gh pr comment`.
- Prefer updating an existing PR on the current branch instead of opening a duplicate.
- If an explicit PR reference is provided, inspect it first and work only when its head branch matches the current branch.
- Before committing, limit the staged set to the docs changes for this sync task.

## PR Body

- Title: `docs: sync <locale> for <path-under-docs/en>`
- Sections, in order: `## Source Pages`, `## Updated Pages`, `## Related Updates`, `## Notes`
- `Related Updates`: `- none` or a short list with reasons.
- `Notes`: only when review risk or translation risk matters.
- If the caller asks for PR-body-only output, return only the PR body text.
- If you add a signature tail, use the exact final line `Submitted by kuku AGENT.`.
