# 使用 Skill

Skill 把可复用的工作流指令按需加载到当前 Session 中。

## 询问 kuku 用法

配好模型后，可以直接问 Agent「怎么恢复上一次会话？」。内置的 `kuku-docs` Skill 覆盖模型配置、会话恢复、Skills、CLI 和 Rust SDK。Agent 可以直接查询运行中的服务随附的文档，也可以按需加载该 Skill 的指令。按 `F1` 或输入 `/help` 可再次找到入口。

要显式调用，从 `/` 菜单选择 `kuku-docs`，在可编辑的 `/skill:kuku-docs` 前缀后输入问题并发送。从脚本或终端运行时，可在位置参数里使用同一前缀。

```bash
kuku run "/skill:kuku-docs 怎样确认服务加载了哪份配置？"
```

Skill 提供用法和出处。你要求执行操作时，Agent 使用运行时提供的工具和权限模式。[内置资料](../reference/skill-format.md#skill-资料) · [工具权限](../how-it-works/permissions.md)

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

模型用 `list_skills` 或 `search_skills` 查阅当前的名称与简介，用 `use_skill` 加载指令。Agent 加载或用户明确选用后，Skill 正文才进入模型上下文。

也可以在 `kuku run` 里直接用斜杠前缀指定某个 Skill——kuku 会加载它，并把后面的文本作为你的提示词：

```bash
kuku run "/skill:tdd implement login"
```

单次运行想彻底关闭 Skill 工具和斜杠加载：

```bash
kuku run --no-skills "task"
```

Skill 只用来指导模型怎么做事，不会扩大权限，也不能绕过工具审批规则。
