# Why kuku

kuku is a Rust SDK for work that can span several model requests, tools or Agents. Embed it in an application or use the terminal, script or browser clients of its HTTP server.

## Inspect a result

Every Session has a journal of submitted inputs, prepared model requests, tool actions and settled results. When an Agent edits a file or runs a command, you can compare the recorded action with the actual workspace. Use `kuku show SESSION_ID` to inspect a result and `kuku events SESSION_ID` to read the underlying facts. [Manage Sessions](../guides/manage-sessions.md)

## Continue a task

A Session can be reopened. A named Agent keeps its own context, waiting requests and retained output. Another Agent in that Session can work independently on the same files. The server lets an HTTP client disconnect while execution continues. [File-native execution](file-native-model.md) · [Run the server](../guides/run-the-server.md)
