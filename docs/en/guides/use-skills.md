# Use Skills

Skills load reusable task instructions into the current Session on demand.

## Add a Skill

Create a directory containing a `SKILL.md` file in either location:

- `~/.kuku/skills/<name>/` — personal workflows
- `<workspace>/.kuku/skills/<name>/` — repository-specific workflows

With auto-discovery enabled, kuku also scans other user and project dot-directories for `skills/` (such as `.claude/skills/`). If a Skill needs to ship together with hooks or other extension assets, bundle it under `.kuku/packages/`. [Skill format](../reference/skill-format.md) · [Package format](../reference/package-format.md)

## Check discovery

```bash
kuku skills list
kuku skills show <name>
```

If a Skill does not show up, check `[discovery]` in [Config](../reference/config.md).

## Use Skills during a run

By default, the model can browse and load Skills using `list_skills`, `search_skills`, and `use_skill`.

You can also invoke a Skill directly from `kuku run` with a slash prefix — kuku loads the named Skill and passes the rest of the string as your prompt:

```bash
kuku run "/tdd implement login"
```

To disable all Skill tools and slash-prefixed loading for one run:

```bash
kuku run --no-skills "task"
```

Skills guide how the model approaches work, but never expand permissions or bypass tool approvals.
