# File-native execution

Each Session has an `events.jsonl` journal and a directory for retained command output. The workspace contains the project files that tools read or change. The local service stores Sessions under kuku home by default. Local clients can select another service data directory with `--data-dir`. SDK callers set their own storage root. [File layout](../reference/file-layout.md)

## What an Agent remembers

Initial context includes the selected Prompt assets, Agent instructions, project instructions and Memory. The SDK records what the model actually received. On later Steps it reuses the committed prefix and adds newly admitted input and tool results. Editing `AGENTS.md` or `memory.md` after a run leaves the earlier conversation reconstructible. Each Agent has its own context even when two Agents work in the same directory. [Prompt assembly](../architecture/prompt-assembly.md)

## Reading and recovery

A client can read committed events by cursor or inspect an Agent's latest settled output. Live text fragments help display work in progress. If an observer disconnects, committed results remain available in the Session. Reopening repairs an incomplete journal tail and restores pending questions or authorizations. Work interrupted after an external action may have an unknown outcome.

The HTTP server owns execution for its clients, so closing a client leaves an active server task running. The current local CLI owns its own Session while it runs. [Sessions](sessions.md) · [Run the server](../guides/run-the-server.md)
