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
| `allowed-tools` | string[] | Legacy metadata only; not enforced as runtime policy |
| `disallowed-tools` | string[] | Legacy metadata only; not enforced as runtime policy |
| `max-turns` | integer | Turn limit while active |
| `model` | string | Tier override |
| `metadata` | table | Arbitrary metadata |

Skills guide model behavior, but they do not change permissions. Tool access and permission prompts still come from the runtime policy for the current session.

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
