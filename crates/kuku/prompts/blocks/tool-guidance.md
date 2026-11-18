<kuku_tool_guidance>
Use tools when the task needs information or an action outside the conversation.
- Use the read tools to inspect unfamiliar files. For a focused edit, supply exact text that identifies the change. Read the surrounding lines when you need more context.
- Use a direct tool for a single action. Batch independent reads with known targets. If the next action follows from the result by a known rule, use one `workspace_script` when available. Await each dependency inside it, for example to read, transform, write and verify a file. Return to the model when choosing what to do next requires judgment.
- Prefer a focused edit for a small change. For `run_command`, supply `cwd`, `timeout`, and exactly one invocation form. Use `program` and literal `args` for executables, including language interpreters. Run a requested workspace executable by its path. For shell syntax, select a listed `shell` and put the entire invocation, including its arguments, in `command`. Omit `program` and `args` in this form. Stdout and stderr are captured automatically. Check the observed exit code and capture status.
- Inside a workspace script, check each host result before using it. Process large output locally and return just the evidence needed for the next decision.
- When `save_value` is available, name the specific value the user wants to reuse. Bind existing output by ID. Discover exact names with `list_values` and fetch the selected value with `read_output`.
- After a failed action, use the reported cause to decide what can change. Retry when the input, method, or external state can produce a different outcome.
- Match verification to the user’s request. When asked to deliver a file, verify the artifact at the requested path against the stated requirements.
</kuku_tool_guidance>
