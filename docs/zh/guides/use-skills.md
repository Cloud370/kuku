# 使用 Skill

Skill 把可复用的工作流指令按需加载到当前 Session 中。

## 添加 Skill

在以下任一路径建一个包含 `SKILL.md` 的目录：

- `~/.kuku/skills/<name>/` — 个人工作流
- `<workspace>/.kuku/skills/<name>/` — 仓库专用工作流

开启自动发现后，kuku 还会扫描其他用户级和项目级点目录下的 `skills/`（例如 `.claude/skills/`）。如果 Skill 需要和 Hook 或其他扩展资源一起打包，放到 `.kuku/packages/` 下。[Skill 格式](../reference/skill-format.md) · [Package 格式](../reference/package-format.md)

## 检查是否被发现

```bash
kuku skills list
kuku skills show <name>
```

如果列表里没看到，检查 [Config 参考](../reference/config.md) 中的 `[discovery]` 配置。

## 运行时使用 Skill

默认情况下，模型可以通过 `list_skills`、`search_skills` 和 `use_skill` 浏览并按需加载 Skill。

也可以在 `kuku run` 里直接用斜杠前缀指定某个 Skill——kuku 会加载它，并把后面的文本作为你的提示词：

```bash
kuku run "/tdd implement login"
```

单次运行想彻底关闭 Skill 工具和斜杠加载：

```bash
kuku run --no-skills "task"
```

Skill 只用来指导模型怎么做事，不会扩大权限，也不能绕过工具审批规则。
