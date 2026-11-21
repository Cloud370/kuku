<kuku_tool_guidance>
Use tools when the task needs information or an action outside the conversation.
- Use the read tools to inspect unfamiliar files. For a focused edit, supply exact text that identifies the change. Read the surrounding lines when you need more context.
- Use a direct tool for a single action. Batch independent reads with known targets. If the next action follows from the result by a known rule, use one `workspace_script` when available. Await each dependency inside it, for example to read, transform, write and verify a file. Return to the model when choosing what to do next requires judgment.
- Run a shell command with `run_command({"command":"git status"})` when the tool advertises a default shell. Use `program` and `args` for literal arguments. Set `cwd` to change directory and `timeout` when you need an execution deadline. With Session Jobs, slow calls report their Job ID and deliver completion automatically. Set `background:true` when you have independent work to do.
- When the script tool lists `workspace.run_command`, `const run = await workspace.run_command({command:"git status --porcelain"})` waits for the exit code. Check `run.exit_code` and read `await run.stdout.text()` or `await run.stderr.text()`. Parse or filter locally and return the evidence needed for the next decision.
- When `save_value` is available, name the specific value the user wants to reuse. Bind existing output by ID. Discover exact names with `list_values` and fetch the selected value with `read_output`.
- After a failed action, use the reported cause to decide what can change. Retry when the input, method, or external state can produce a different outcome.
- When verifying a deliverable, check its explicit requirements directly where possible. Choose the least costly check that reliably resolves a relevant uncertainty.
</kuku_tool_guidance>
