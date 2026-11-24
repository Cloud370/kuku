---
name: kuku-docs
description: Explain how to use this version of kuku, including setup, models and connections, Sessions, Skills, the CLI, and the Rust SDK.
---

# Use kuku

Identify the running kuku version if known, the entry point the user is using, and what they want to do. Use the bundled guides for this service version and respond in the user's language.

Use `skill_resources` with `skill_name: "kuku-docs"`. For a specific question, read the relevant page directly or search a language directory for a distinctive term. For a broad question, start with `references/docs/en/index.md` or `references/docs/zh/index.md`, then read only the relevant sections. Use the supplied paths and line offsets to follow up. Cite the page and section, using its pinned source URL when available.

Use the quickstart, guides, and reference pages to answer questions about current behavior. Architecture and contributor pages may describe targets and development procedures. When diagnosing an error, use the actual error and settings, give the checks that apply, and explain what to do next.

When the user asks how to do something, explain the steps. When they ask you to do it, use the native tools available for the requested action and follow the runtime's authorization. Consult these references as needed to check a version-specific fact.
