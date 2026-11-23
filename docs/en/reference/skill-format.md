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
| `allowed-tools` | string[] | Legacy metadata only; not enforced as runtime policy |
| `disallowed-tools` | string[] | Legacy metadata only; not enforced as runtime policy |
| `max-turns` | integer | Turn limit while active |
| `model` | string | Tier override |
| `metadata` | table | Arbitrary metadata |

Skills guide model behavior and never alter runtime tool permissions.

## Runtime snapshotting

At the start of each Turn, kuku snapshots discovered Skills into the Session journal (including package Skills when `plugin.enabled = true`):

- `list_skills`, `search_skills`, and `use_skill` all read from the current Turn's snapshot rather than live disk.
- Resuming a Turn restores its recorded snapshot.
- Edits on disk take effect on the next fresh Turn.

## Path resolution

When a Skill is loaded, kuku prepends a location marker:

```markdown
<!-- loaded: /home/user/.kuku/skills/tdd -->
```

Relative paths inside the Skill instructions resolve against that directory.
