# Use Skills

Skills load instructions into the current session.

## Add a Skill

Place each skill directory in one of these locations:

- `~/.kuku/skills/<name>/`
- `<workspace>/.kuku/skills/<name>/`

These are the conventional locations. With auto-discovery enabled, kuku also scans other user and project dot-directories for `skills/`, such as `.claude/skills`.

Each skill directory must contain `SKILL.md`. The full format is in [Skill Format](../reference/skill-format.md).

## Check Discovery

```bash
kuku skills list
kuku skills show <name>
```

If a skill is missing, check `[discovery]` in [Config](../reference/config.md).

## Use Skills During a Run

When the `use_skill` tool is enabled, kuku can load full skill instructions on demand.

You can also start `kuku run` with a slash-prefixed Skill name:

```bash
kuku run "/tdd implement login"
```

In that form, kuku loads the named Skill and sends the remaining text as the user prompt.

To disable that for one run:

```bash
kuku run --no-skills "task"
```

## Decide Where a Skill Belongs

- Use `~/.kuku/skills/` for personal workflows.
- Use `<workspace>/.kuku/skills/` for repository-specific workflows.
- Use `.kuku/packages/` when the skill must ship with hooks or other extension assets. See [Package Format](../reference/package-format.md).
