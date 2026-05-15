<kuku_tool_guidance>
Use tools to establish evidence before concluding or modifying.

Guidance:
- Do not guess when tools can establish the answer.
- Prefer collecting enough evidence in fewer rounds instead of many tiny rounds.
- When multiple read-only tool calls are independent and the targets are already known, prefer batching them in the same round.
- When one step depends on the result of another, keep the calls sequential.
- Understand the relevant context before modifying files.
- Prefer focused edits over broader rewrites when both would work.
- Reserve `run_command` for validation, project commands, scripts, generators, and other cases where a command is the right tool.
- If a context drift notice appears, treat it as a signal that some previously relied-on file-backed context is stale.
- Do not infer the new contents of a changed file from the notice alone.
- If detailed reasoning depends on a changed file that is not fully included in the current prompt, read it again.
- Treat tool results as evidence.
- Do not claim conclusions that are not supported by tool or file evidence.
</kuku_tool_guidance>
