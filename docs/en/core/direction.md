# Direction

kuku is a Rust SDK that writes every agent fact into files on disk.

Open it. `grep` it. `diff` it against the last run. Commit it to git. Every question, every tool call, every answer — one line after another, nothing hidden, nothing locked inside an app.

## What it is

- A Rust library that apps build on. Think of it as the runtime, not the app.
- A `session` is a directory. Everything that happens is a line in `events.jsonl` inside it.
- Behavior comes from files you already have: `AGENTS.md`, `CLAUDE.md`, `memory.md`.
- Safety is enforced by the runtime, not by prompt instructions.

## What it is not

| If you're looking for | kuku is not that |
|------------------------|-------------------|
| A CLI you install and chat with | kuku is a library. CLI is a host app built on it. |
| A drop-in Claude Code replacement | Same familiar loop — ask, tools, answer — but state is files, not a long-running process. |
| A plugin store | Extension points exist for host apps. The SDK itself stays small. |

## What stays out of the core loop

Request inspection, transcript export, handoff, resume — derived views, not runtime.
Package, Hook, MCP — extension boundaries, not core.
TUI, WebUI — host apps that present SDK facts.

`subagent` is in the SDK but is a tool-backed mechanism — a child `session`, not a separate platform.

## What comes next

Skills are native to the SDK (load .md files, inject context). Everything else that extends the agent — MCP, hooks, custom tools — enters through the extension/package system, not the core.

Host apps are independent binaries. `terminal` exists today. `server` (HTTP API with NDJSON streaming) and `web` (frontend SPA) are planned. `tauri` (desktop) follows. Each host calls the SDK directly; no host embeds another.

Detailed design: [apps.md](apps.md) · [skills.md](skills.md) · [evolution.md](evolution.md).

See [architecture.md](architecture.md) for how the pieces fit together, [agent-loop.md](agent-loop.md) for how turns execute.
