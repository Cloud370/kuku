# Skills

<!-- status: implemented -->

A skill is a packaged capability (instructions, scripts, references) that extends the current session. Skills follow the [Agent Skills specification](https://agentskills.io/specification).

## Skills vs agents

| | Skill | Agent |
|---|---|---|
| Execution | Injects into current session | Spawns child session |
| Tools | Current session's tool set | Constrained child tool set |
| Turn limit | Shares parent session lifecycle | Has `max_turns` |
| Use case | "Follow this workflow" | "Do this independently" |

A skill adds knowledge. An agent adds an executor.

## Directory structure 
```text
.kuku/skills/
├── tdd/
│   ├── SKILL.md            # required: instructions
│   ├── references/         # optional: detailed docs, loaded on demand
│   ├── scripts/            # optional: executable code
│   ├── examples/           # optional: working examples
│   └── assets/             # optional: templates, schemas
└── code-review/
    └── SKILL.md
```

User-level skills: `~/.kuku/skills/`.
Project-level skills: `.kuku/skills/`.

## SKILL.md format

```yaml
---
name: tdd
description: >
  Write tests before implementation. Follow red-green-refactor.
---

# TDD

Before writing any code:

1. Write a failing test
2. Write minimal code to pass
3. Refactor

Run the test suite: `scripts/run-tests.sh`
```

### Required frontmatter

| Field | Rule |
|-------|------|
| `name` | 1-64 chars, lowercase letters, numbers, hyphens. Must match directory name. |
| `description` | 1-1024 chars. What the skill does AND when to use it. |

### Optional frontmatter

| Field | Type | Purpose |
|-------|------|---------|
| `allowed-tools` | string[] | Tools the skill may use without permission prompts |
| `disallowed-tools` | string[] | Tools the skill must not use |
| `max-turns` | int | Turn limit when skill is active |
| `model` | string | Override model tier for this skill |
| `metadata` | map | Arbitrary key-value pairs |

## Progressive disclosure 
Three-stage loading to minimize context usage:

| Stage | Content | When loaded |
|-------|---------|-------------|
| Metadata | `name` + `description` (~100 tokens) | Session startup → injected into `runtime_context` |
| Instructions | Full `SKILL.md` body (<5,000 tokens) | Skill triggered |
| Resources | `references/`, `scripts/`, `examples/` | Model reads them on demand via `read_file` or `run_command` |

The model sees the catalog at startup. It calls `use_skill` to load full instructions. It uses existing tools (`read_file`, `run_command`) to access resources.

## SkillRegistry
`SkillRegistry` loads skill definitions via pattern-based discovery scanning. Same pattern as `SubagentRegistry`.

```rust
let registry = SkillRegistry::builder()
    .build_with_discovery(&workspace, &discovery_config)?
    .build();
```

Catalog is injected into `runtime_context`:

```xml
<kuku_skills>
  tdd: Write tests before implementation. Follow red-green-refactor.
  code-review: Review code for correctness, edge cases, and evidence.
</kuku_skills>
```

## use_skill tool 
A built-in tool that loads a skill's full instructions into the current session.

```json
{
  "name": "use_skill",
  "description": "Load a skill into the current session",
  "input_schema": {
    "type": "object",
    "properties": {
      "skill_name": { "type": "string" }
    },
    "required": ["skill_name"]
  }
}
```

On invocation:

1. Load `SKILL.md` body
2. Inject base directory comment: `<!-- skill_dir: /path/to/skill -->`
3. Append to current turn's context

The model reads the injected instructions, then uses `run_command` to execute scripts and `read_file` to access references. Scripts are not registered as tools.

## Path resolution

Skills use relative paths. The SDK injects the skill's absolute base directory when loading instructions:

```markdown
<!-- skill_dir: /home/user/.kuku/skills/tdd -->
```

The model resolves relative paths against this base directory. No template variables.

## Host integration 
Hosts can trigger skills directly via slash commands:

```text
User types: /tdd implement login
Host prepends: [SKILL:tdd] implement login
SDK recognizes prefix → loads skill → injects context
```

The host owns command parsing and UI. The SDK owns skill loading and context injection.

| Host | Trigger mechanism |
|------|-------------------|
| terminal | `/skill-name` in prompt |
| server | `skills` field in `POST /runs` body |
| tauri | Slash command UI |

## Relationship to extension system 
Skills are native to the SDK because they are simple (load .md files, inject context). The package system bundles skills alongside hooks and MCP servers. The skill format defined here is a subset of the package format.

```text
.kuku/skills/tdd/SKILL.md           ← stage 1: bare skill
.kuku/packages/tdd-suite/kuku.toml  ← stage 2: skill inside a package
```

MCP is not part of the SDK core. It is an extension loaded through the package system.
