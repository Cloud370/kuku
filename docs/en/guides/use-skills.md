# Use Skills

Skills load reusable task instructions into the current Session on demand.

## Ask about kuku

Once you have configured a model, ask the Agent a usage question such as `How do I resume my last Session?` The built-in `kuku-docs` Skill covers model setup, Session recovery, Skills, CLI, and the Rust SDK. The Agent can load the Skill and look up instructions from the docs bundled with the running service. See `F1` or `/help` to find this entry again.

To choose it explicitly, open the `/` menu and select `kuku-docs`. Type your question after the editable `/skill:kuku-docs` prefix and send it. From a script or terminal, pass the same prefix in the positional prompt.

```bash
kuku run "/skill:kuku-docs How do I check which config my service loaded?"
```

The Skill supplies instructions and sources. The Agent uses the current runtime's tools and permission mode for any operations you ask it to perform. [Built-in resources](../reference/skill-format.md#skill-resources) · [Tool permissions](../how-it-works/permissions.md)

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

By default, the model sees Skill names and descriptions and can browse and load their instructions using `list_skills`, `search_skills`, and `use_skill`. A Skill's instructions enter the model context when the Agent loads it or you select it explicitly.

You can also invoke a Skill directly from `kuku run` with a slash prefix — kuku loads the named Skill and passes the rest of the string as your prompt:

```bash
kuku run "/skill:tdd implement login"
```

To disable all Skill tools and slash-prefixed loading for one run:

```bash
kuku run --no-skills "task"
```

Skills guide how the model approaches work, but never expand permissions or bypass tool approvals.
