# Use Skills

Skill 会把指令加载到当前 Session 中。

## Add a Skill

把每个 skill 目录放到以下位置之一：

- `~/.kuku/skills/<name>/`
- `<workspace>/.kuku/skills/<name>/`

这些是约定位置。启用 auto-discovery 时，kuku 也会扫描其他用户级和项目级 dot-directory 中的 `skills/`，例如 `.claude/skills`。

每个 skill 目录都必须包含 `SKILL.md`。完整格式见 [Skill Format](../reference/skill-format.md)。

## Check Discovery

```bash
kuku skills list
kuku skills show <name>
```

如果某个 skill 缺失，请检查 [Config](../reference/config.md) 中的 `[discovery]`。

## Use Skills During a Run

当默认 Skill Tool surface 启用时，kuku 可以：

- 使用 `list_skills` 浏览当前目录快照
- 使用 `search_skills` 查找相关工作流
- 使用 `use_skill` 按需加载完整的 skill 指令

你也可以用带斜杠前缀的 Skill 名称启动 `kuku run`：

```bash
kuku run "/tdd implement login"
```

在这种形式下，kuku 会加载指定的 Skill，并把剩余文本当作用户 prompt 发送。

如果要在某一次运行中禁用这个能力：

```bash
kuku run --no-skills "task"
```

`--no-skills` 会禁用默认的 Skill Tool surface，因此这次运行中 `list_skills`、`search_skills`、`use_skill` 以及斜杠前缀的 Skill 加载都不可用。

Skill 可以指导模型如何工作，但不会扩展权限，也不会绕过 Tool 审批规则。

## Decide Where a Skill Belongs

- `~/.kuku/skills/` 适合个人工作流。
- `<workspace>/.kuku/skills/` 适合仓库专用工作流。
- 当 skill 需要随 hooks 或其他扩展资源一起发布时，使用 `.kuku/packages/`。参见 [Package Format](../reference/package-format.md)。
