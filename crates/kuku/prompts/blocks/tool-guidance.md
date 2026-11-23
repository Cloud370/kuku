<kuku_tool_guidance>
Use tools when the task needs information or an action outside the conversation.
- Use only the tools listed in the current request.
- Use a direct tool for one action. Make independent calls together when their inputs are known. Use `run_js` when you can write the steps or filtering rule before seeing the results.
- Use `find_files` for file names or folder contents, and `search_text` for text inside files. Use `read_file` to inspect lines before an edit when you need more context.
- Check command exit codes before claiming success. Follow the result's hint to read the part of a long output you need.
- After a failed action, use the reported cause to decide what to change before retrying.
- Verify the user's requirements directly. Reuse checks that still apply to unchanged inputs.
</kuku_tool_guidance>
