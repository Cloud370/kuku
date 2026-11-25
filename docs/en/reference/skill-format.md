# Skill format

## Locations and layout

- User scope: `~/.kuku/skills/<name>/`
- Project scope: `<workspace>/.kuku/skills/<name>/`

```text
<skill>/
├── SKILL.md
├── references/
├── scripts/
├── examples/
└── assets/
```

Only `SKILL.md` is required. The other directories are not preloaded automatically; they stay on disk for instructions or tools to reference.

## `SKILL.md` format

YAML frontmatter followed by Markdown instructions:

```markdown
---
name: tdd
description: Write tests before implementation. Follow red-green-refactor.
---

# TDD

Before writing any code:

1. Write a failing test
2. Write minimal code to pass
3. Refactor
```

## Frontmatter fields

Required:

| Field | Rule |
|---|---|
| `name` | 1–64 chars (lowercase letters, digits, hyphens); must match the directory name |
| `description` | 1–1024 chars; explains what the Skill does and when to use it |

Optional:

| Field | Type | Meaning |
|---|---|---|
| `allowed-tools` | string[] | Parsed metadata about suggested tools |
| `disallowed-tools` | string[] | Parsed metadata about excluded tools |
| `max-turns` | integer | Parsed metadata |
| `model` | string | Parsed metadata |
| `metadata` | table | Arbitrary metadata |

Skill metadata is descriptive. The Agent's configuration, runtime limits and host policy control model selection, execution budgets, and tool authorization.

## Runtime snapshotting

At the start of a fresh Turn, kuku discovers Skills (including package Skills when `plugin.enabled = true`). The Session journal records only a fingerprint; unchanged catalogs share one immutable snapshot in the service data directory under `skill-catalogs/`. Keep that directory with the Session journal when moving or backing up the service data.

- A default Session’s initial model context lists every available Skill name. Descriptions share an estimated 8,192-token budget. Kuku keeps descriptions intact when they fit, marks shortened descriptions, and never removes a name to fit that budget. Instructions are still loaded on demand.
- At the first request of a new Turn, the runtime adds one notice for catalog additions, revisions, and removals since the last model-visible catalog. It leaves the frozen prefix intact. The same Turn keeps its catalog; a sort-only change does not send a notice.
- `list_skills`, `search_skills`, and `use_skill` all read from the current Turn's snapshot rather than live disk.
- Resuming a Turn restores its recorded snapshot.
- Edits on disk take effect on the next fresh Turn.

A steering input is checked against the active Turn’s catalog before delivery. If the selected Skill was installed after that Turn started, the request is rejected without interrupting the active Turn. Send it as a new Turn instead.

A Skill is loaded only when its exact instructions are present in the Agent’s context. If that revision is already present, `use_skill` reports reuse instead of repeating the instructions. If the file changes or those instructions leave context, the Agent can load the applicable revision again.

## Path resolution

When a Skill is loaded, kuku prepends a location marker:

```markdown
<!-- loaded: /home/user/.kuku/skills/tdd -->
```

Use the location marker to find a user or project Skill's supporting files. Relative arguments to the workspace file tools resolve from the Agent's workspace, and those tools apply normal workspace path authorization.

## Skill resources

`kuku-docs` provides the embedded `docs/` tree under `references/docs/`. The read-only `skill_resources` tool can read it without first loading the instructions. Kuku retains the bundled version on first load or resource access, or before an active Turn pauses for a resumable permission request. The paused Turn can then still access its pinned version after a service upgrade. Start with `references/docs/en/index.md` or `references/docs/zh/index.md` for an overview, or read the page directly for a specific question.

| Action | Arguments | Result |
|---|---|---|
| `list` | Optional directory `path`, `offset`, `limit` | Recursively lists up to 20 paths per page |
| `search` | Literal case-insensitive `query`, optional directory `path`, `offset`, `limit` | Matches with file paths and line numbers, up to 20 per page |
| `read` | File `path`, optional 1-based line `offset` and `limit` | Bounded Markdown lines, up to 200 per call |

Pass `skill_name: "kuku-docs"` for each call. The tool applies normal read authorization and confines paths to this Skill's resource directory. Results identify the resource version and source revision. Links between bundled pages remain relative. Links to other repository files point to the corresponding source commit.

Resources decompress on the first resource read and publish only after integrity verification. The cache is shared across Sessions in the service's data directory under `skill-resources/kuku-docs/`, keyed by content fingerprint. [Using built-in help](../guides/use-skills.md#ask-about-kuku)
