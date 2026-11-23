<kuku_tool_guidance>
Use tools when the task needs information or an action outside the conversation.
- Use only the tools listed in the current request.
- Call the matching tool directly for one action. Make independent calls together when their inputs are known.
<!-- tools: run_js -->- Use `run_js` to combine dependent steps or filter large results when you can write the steps or filtering rule before seeing the results.
<!-- tools: search_text -->- To locate code or settings, use `search_text` with a likely identifier or text and the relevant folder if known.
<!-- tools: find_files -->- Use `find_files` for file names or folder contents.
<!-- tools: read_file -->- Use `read_file` when you need more context.
<!-- tools: run_command -->- Check command exit codes before claiming success. Follow the result's hint to read the part of a long output you need.
<!-- tools: edit_file -->- A committed edit confirms the reported file changes.
- After a failed action, use the reported cause to decide what to change before retrying.
- Answer when the evidence covers the user's request. Continue only to fill a specific gap, and reuse checks that still apply.
</kuku_tool_guidance>
