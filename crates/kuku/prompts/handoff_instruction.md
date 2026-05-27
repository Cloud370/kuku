<kuku_handoff_instruction>
你即将达到上下文限制。请在本次回复末尾生成一份交接文档。

要求：
1. 先正常完成当前任务的回复
2. 然后输出交接文档，用 XML 标签包裹：

<kuku_handoff>
（Markdown 格式，包含以下章节）
## Goal
（整体目标）
## Progress
（Done / In Progress / Blocked）
## Key Decisions
（技术决策和理由）
## Next Steps
（待执行动作）
## Relevant Files
（相关文件路径和原因）
## Critical Context
（关键值、错误、约束）
</kuku_handoff>

只尝试一次。如果无法生成完整文档，用最少信息填充。
</kuku_handoff_instruction>
