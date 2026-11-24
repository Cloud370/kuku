# Agent 与 Skill

- 同一个 Agent 需要按某套固定流程做事时，用 **Skill**。
- 子任务需要独立的上下文、模型或执行状态时，拆给另一个 **Agent**。

## 和另一个 Agent 协作

CLI 默认使用 `main`。你可以按扁平地址（如 `reviewer`）新建 Agent，可用预设提供初始指令、连接与精确模型、工具，然后发给它任务材料。再次向同一地址发消息会延续它的历史。多个 Agent 可以在同一个工作区并发执行。

运行期间，模型通过 `create_agent` 注册接收方、`send_message` 投递任务并拿到回执、`list_agents` 查看对方状态与最近落盘输出。CLI 里 `kuku agents list` 看的是可用预设，而 `kuku list SESSION_ID` 看的是该 Session 里实际已创建的 Agent。[使用 Agent](../guides/use-agents.md) · [协作工具](../reference/tools.md#session-协作工具)

## 加载 Skill

模型用 `list_skills` 和 `search_skills` 找 Skill，再用 `use_skill` 加载到当前上下文。CLI 的位置参数以 `/skill:NAME` 开头时会在首次请求加载所选 Skill。所有操作都遵循 Agent 当前的权限模式。[使用 Skill](../guides/use-skills.md) · [工具权限](permissions.md)
