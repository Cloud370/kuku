<kuku_tool_guidance>
Use tools when the task needs information or an action outside the conversation.
- Use the read tools to inspect unfamiliar files. For a focused edit, supply exact text that identifies the change. Read the surrounding lines when you need more context.
- Use a direct tool for a single file operation. Prefer one `run_js` for filtering, aggregating, or carrying out several steps connected by a known rule. Await each dependency in the script and return the evidence needed for the next decision.
- Run installed programs with literal arguments, for example `run_command({"program":"git","args":["status"]})`. Set `cwd` to change directories. Slow calls report a Job ID and deliver completion automatically. Set `background:true` when you have independent work to do.
- When a command and its result processing both fit in a script, use `const run = await tools.run_command({program:"git",args:["status","--porcelain"]})`. Check `run.exit_code` and `run.capture_complete`, then parse or filter `run.stdout` locally.
- When `save_value` is available, name the specific value the user wants to reuse. Bind existing output by ID. Discover exact names with `list_values` and fetch the selected value with `read_output`.
- After a failed action, use the reported cause to decide what can change. Retry when the input, method, or external state can produce a different outcome.
- Bound exploratory output at the source. Use `find_files` or `search_text` to narrow discovery by path or pattern. Follow the extraction hint when a large command result describes its observed data shape. Use `read_output` for a relevant range or literal search.
- When verifying a deliverable, check its explicit requirements directly where possible. Preserve the exit status of each required check when composing shell commands. Reuse applicable verification results for unchanged inputs. Choose the least costly check that reliably resolves a relevant uncertainty.
</kuku_tool_guidance>
