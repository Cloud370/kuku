---
name: docs-translate
description: Translate a specified English kuku docs page to its mirrored locale path when the repository translation runner invokes this Skill.
---

# Docs Translate

Read `docs/AGENTS.md`, the specified English source page, and the target page if present. Refer to nearby translated pages for established terminology when needed.

Translate into the requested locale and write the specified target page. Keep the document's structure, links, code spans, commands, paths, and config keys. Translate visible labels and preserve canonical product terms such as `Agent`, `Skill`, `Session`, `Memory`, `Prompt`, and `Server`. Use natural, concise technical prose.

Limit edits to the specified target. If its text already matches the source, leave the page as it is. Return a short statement of the outcome. The caller handles Git and the PR.
