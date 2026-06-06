# Agents and Skills

Agent 和 Skill 都会扩展模型可以做的事情，但方式不同。

## The difference

| | Skill | Agent |
|---|---|---|
| Execution model | 将指令注入当前 Session | 生成一个子 Session |
| Tools | 使用当前 Session 的 Tool 集合 | 使用受限的子 Tool 集合 |
| Lifetime | 与父 Session 共享生命周期 | 在子 Session 结束或达到轮次上限时结束 |
| Best for | 工作流指导或打包知识 | 独立的委派工作 |

Skill 增加的是指令。Agent 增加的是另一个执行者。

## Skills

Skill 是从 Skill 目录中发现的、基于 markdown 的能力。模型在当前 Session 中需要工作流、参考资料或打包行为时，会加载某个 Skill。

Skill 不会创建独立的 Session 状态。它们通过加入指令和可选资源来改变当前 Session。

启用 skills 时，运行时会暴露默认的 Skill Tool surface：`list_skills`、`search_skills` 和 `use_skill`。

## Agents

Agent 是从用户目录或项目目录中发现的 subagent 定义。模型使用 Agent 时，kuku 会创建一个子 Session，并在那里运行同样的 Agent Loop。

这个子 Session 具有：

- 自己的事件日志
- 会把权限请求回传给父级 run
- 有限的深度预算
- 自己的轮次上限

嵌套 Agent 委派最多只允许 `parent -> child -> grandchild`。如果某个子 Session 再继续生成更深一层的 Agent，运行时会用 `blocked: maximum subagent depth (2) reached` 阻止这次调用。

## Permissions and inheritance

Subagent 的权限请求会回传给父级 run 来做决定。硬性保护规则仍然适用。

Skill 也不会绕过权限。它们可以指导行为，但运行时仍然决定哪些 Tool 调用被允许。

权限模型见 [Permissions](permissions.md)。

## Where each fact belongs

- 本页解释运行时层面的关系。
- 使用流程应放在 `guides/`。
- 文件格式应放在 `reference/`。
- 包和加载器的内部细节应放在 [Extension Runtime](../architecture/extension-runtime.md)。
