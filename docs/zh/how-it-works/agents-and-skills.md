# Agent 与 Skill

Agent 是一个 Session 中可以独立收件和执行任务的成员。Skill 为加载它的 Agent 提供指令或资源。同一个 Agent 需要按某种流程工作时，使用 Skill。任务需要独立上下文、模型或执行状态时，使用另一个 Agent。

## 和另一个 Agent 协作

CLI 默认使用 `main`。为新 Agent 选择 `reviewer` 这样的扁平地址，可以选用预设来设置初始指令、模型档和工具。把执行任务所需的材料发给它。再次使用同一地址会延续该 Agent 自己的历史。同一个工作目录中可以有多个 Agent 同时执行。

模型工具 `create_agent` 创建接收方，`send_message` 向已有地址发送输入并返回回执。接收方之后产生的结果可以通过 `list_agents` 或 Session 查询查看。CLI 的 `kuku agents list` 列出可选预设，`kuku list SESSION_ID` 列出该 Session 中实际存在的 Agent。[使用 Agent](../guides/use-agents.md) · [协作工具](../reference/tools.md#session-协作工具)

## 加载 Skill

通过 `list_skills` 和 `search_skills` 查找 Skill，通过 `use_skill` 将其加载到当前 Agent 的上下文。CLI 输入以已识别的 `/skill-name` 开头时，也可以为首个请求加载该 Skill。加载之后请求执行的动作仍遵循相同的工具授权规则。[使用 Skill](../guides/use-skills.md) · [工具权限](permissions.md)
