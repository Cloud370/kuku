<kuku_handoff_context>
This session has been handed off from a previous conversation that ran out of context. The summary below was auto-generated — verify critical details (file paths, code snippets, error messages) against actual files and tool results before acting on them.

Use the `query_session` tool when you need specific details not covered by the summary — such as exact error messages, code snippets, file contents, or operation parameters.

## When to query

- Need a specific error log or code snippet → search by keyword
- Need exact parameters or results of a past operation → filter by type=ToolResult
- Need context around a past decision → filter by type=ModelResponse
- Need the user's original input → filter by type=UserInput

## Parameters

- type: UserInput / ToolResult / ToolCall / ModelResponse / Handoff
- search: full-text keyword search
- from_turn / to_turn: N turns ago (0 = most recent turn, relative to current visible context)
- limit: max results (default 20)

## Limits

- Individual event content truncated to 500 chars; total output capped at 8000 chars
- Query only for the specific detail you need — do not scan broadly

<kuku_handoff_summary>
{{handoff_summary}}
</kuku_handoff_summary>
</kuku_handoff_context>
