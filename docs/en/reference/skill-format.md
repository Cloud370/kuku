# Skill Format

## Locations

- User scope: `~/.kuku/skills/<name>/`
- Project scope: `<workspace>/.kuku/skills/<name>/`

## Required Layout

```text
<skill>/
├── SKILL.md
├── references/
├── scripts/
├── examples/
└── assets/
```

Only `SKILL.md` is required.

## `SKILL.md` Shape

`SKILL.md` uses YAML frontmatter followed by Markdown instructions.

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

## Frontmatter Fields

Required:

| Field | Rule |
|---|---|
| `name` | 1-64 chars, lowercase letters, numbers, hyphens; must match the directory name |
| `description` | 1-1024 chars; what the skill does and when to use it |

Optional:

| Field | Type | Meaning |
|---|---|---|
| `allowed-tools` | string[] | Tools the skill may use without prompts |
| `disallowed-tools` | string[] | Tools the skill must not use |
| `max-turns` | integer | Turn limit while active |
| `model` | string | Tier override |
| `metadata` | table | Arbitrary metadata |

## Loading Model

Skills load in three stages:

1. metadata at session startup
2. full `SKILL.md` when the skill is used
3. `references/`, `scripts/`, `examples/`, and `assets/` on demand

## Path Resolution

When a Skill is loaded, the runtime prepends a comment such as:

```markdown
<!-- loaded: /home/user/.kuku/skills/tdd -->
```

Relative paths used in the Skill instructions resolve from that Skill directory.
