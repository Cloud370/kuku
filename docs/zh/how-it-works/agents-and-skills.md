# Agent 与 Skill

- 同一个 Agent 需要按某套固定流程做事时，用 **Skill**。
- 子任务需要独立的上下文、模型或执行状态时，拆给另一个 **Agent**。

## 和另一个 Agent 协作

CLI 默认使用 `main`。你可以按扁平地址（如 `reviewer`）新建 Agent，可选用预设来配好初始指令、Tier 和工具，再把任务所需的材料发过去。再次向同一地址发消息会延续它自己的历史。多个 Agent 可以在同一个工作区里并发执行。

运行期间，模型通过 `create_agent` 注册接收方、`send_message` 投递任务并拿到回执、`list_agents` 查看对方状态与最近落盘输出。CLI 里 `kuku agents list` 看的是可用预设，而 `kuku list SESSION_ID` 看的是该 Session 里实际已创建的 Agent。[使用 Agent](../guides/use-agents.md) · [协作工具](../reference/tools.md#session-协作工具)

## 加载 Skill

模型用 `list_skills` 和 `search_skills` 找 Skill，用 `use_skill` 加载进当前上下文；CLI 输入以已识别的 `/skill-name` 开头时也会在首轮直接加载。加载 Skill 后发起的任何工具调用，依然遵循完全相同的权限审批规则。[使用 Skill](../guides/use-skills.md) · [工具权限](permissions.md)
