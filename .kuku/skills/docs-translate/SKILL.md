---
name: docs-translate
description: Use only when the user explicitly asks to translate, sync, or update this repository's public Chinese docs mirror from `docs/en/**` to `docs/zh/**`, or when a dedicated docs-translate script or workflow invokes it. Never use for normal code work, normal English docs editing, review, or unrelated repository tasks.
allowed-tools:
  - find_files
  - read_file
  - search_text
  - run_command
  - edit_file
  - write_file
---

# Docs Translate

This skill assumes the caller has already decided this is a manual docs translation run for this repository.

## Working Loop

Follow this loop:

1. Read `docs/AGENTS.md` before changing translated docs.
2. Inspect repo state with `run_command` using focused Git commands.
3. Identify the English source pages under `docs/en/**` that actually need translation work.
4. Read the source pages and their matching `docs/zh/**` pages before editing.
5. Translate conservatively into the mirrored Chinese paths.
6. If the task includes formal sync or PR update, use `gh` through `run_command` only after the file changes are ready.
7. Summarize which pages were updated and why, especially for any related pages beyond the direct mirror files.

## Efficiency Guidance

Prefer the smallest useful command and the fewest tool rounds.

- Start with focused Git inspection:
  - `git status --short`
  - `git diff --name-only -- docs/en`
  - `git diff --stat -- docs/en docs/zh`
- When the changed set is already known, batch reads instead of many tiny single-file reads.
- Read the English page and its mirrored Chinese page in parallel when possible.
- If multiple independent docs pages need translation, process them in batches, not one shell round per file.
- Batch related edits when they belong to the same page set, but do not bundle unrelated rewrites.
- For long command output, prefer focused commands first. If a command is still noisy, it is acceptable to pipe to `tail -n` for the failure or final summary section.
- Use `gh` only for explicit PR work, and prefer direct subcommands such as:
  - `gh pr view`
  - `gh pr create`
  - `gh pr edit`
  - `gh pr comment`
- Do not spend turns rediscovering repository structure that is already known from `docs/AGENTS.md` and the mirrored path rule.

## Tool Guidance

- Use `run_command` for repository state and GitHub operations.
- Use `git status --short` to inspect the worktree.
- Use focused `git diff` commands to find relevant English docs changes.
- Use `gh` only when the task explicitly includes PR inspection, creation, or update.
- Use `read_file` and `search_text` to inspect docs content before editing.
- Use `edit_file` or `write_file` only after reading the relevant source and target pages.
- Prefer parallel reads over repeated serial reads when file paths are already known.
- Prefer focused edits over broad rewrites when both would work.

For ordinary local translation-only runs, do not use `gh` unless the user explicitly asks for PR or branch work.

## Translation Rules

- `docs/en/**` is the canonical source.
- Keep the mirrored path structure under `docs/zh/**`.
- Preserve Markdown structure, headings, code spans, commands, paths, config keys, and links.
- Keep canonical product terms in English, including:
  - `Agent`
  - `Skill`
  - `Session`
  - `Memory`
  - `Tool`
  - `Prompt`
  - `Server`
  - `Package`
  - `Hook`
- Prefer the direct corresponding translated page first.
- Additional related-page updates are allowed only when clearly needed for consistency.
- If updating pages beyond the direct corresponding set, explain why in the final summary or PR body.

## Behavior Rules

- Be conservative.
- Do not rewrite large unrelated sections.
- Do not invent new documentation structure.
- Respect the repository docs rules in `docs/AGENTS.md`.
- Treat recent English changes as the main cue, not as permission to rewrite the whole docs tree.
- If a page does not need translation changes, leave it alone.
- If formal sync is requested, keep PR language factual and short.

## PR Body Contract

When a translation run needs a PR body, keep it short and factual.

Include these sections in this order:

1. `## Source Pages`
2. `## Updated Pages`
3. `## Related Updates`
4. `## Notes`

Rules:

- `Source Pages` lists the English pages that drove this translation run.
- `Updated Pages` lists the translated files that changed.
- `Related Updates` is either `- none` or a short bullet list with reasons.
- `Notes` should mention review expectations or translation risk only when needed.
- Do not write marketing language.
- Do not write long explanations.
