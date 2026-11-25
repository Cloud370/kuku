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

At the start of each Turn, kuku snapshots discovered Skills into the Session journal (including package Skills when `plugin.enabled = true`):

- `list_skills`, `search_skills`, and `use_skill` all read from the current Turn's snapshot rather than live disk.
- Resuming a Turn restores its recorded snapshot.
- Edits on disk take effect on the next fresh Turn.

The built-in `kuku-docs` Skill follows the running service's build. When a Turn loads it, kuku retains the resource version with that Turn's snapshot. Resuming that Turn can still read its recorded version after an upgrade or rollback.

## Path resolution

When a Skill is loaded, kuku prepends a location marker:

```markdown
<!-- loaded: /home/user/.kuku/skills/tdd -->
```

Use the location marker to find a user or project Skill's supporting files. Relative arguments to the workspace file tools resolve from the Agent's workspace, and those tools apply normal workspace path authorization.

## Skill resources

`kuku-docs` provides the embedded `docs/` tree under `references/docs/`. After loading the Skill in the current Turn, the Agent can call the read-only `skill_resources` tool. Start with `references/docs/en/index.md` or `references/docs/zh/index.md` for an overview, or read the page directly for a specific question.

| Action | Arguments | Result |
|---|---|---|
| `list` | Optional directory `path`, `offset`, `limit` | Recursively lists up to 20 paths per page |
| `search` | Literal case-insensitive `query`, optional directory `path`, `offset`, `limit` | Matches with file paths and line numbers, up to 20 per page |
| `read` | File `path`, optional 1-based line `offset` and `limit` | Bounded Markdown lines, up to 200 per call |

Pass `skill_name: "kuku-docs"` for each call. The tool applies normal read authorization and confines paths to this Skill's resource directory. Results identify the resource version and source revision. Links between bundled pages remain relative. Links to other repository files point to the corresponding source commit.

Resources decompress on the first resource read and publish only after integrity verification. The cache is shared across Sessions in the service's data directory under `skill-resources/kuku-docs/`, keyed by content fingerprint. [Using built-in help](../guides/use-skills.md#ask-about-kuku)
