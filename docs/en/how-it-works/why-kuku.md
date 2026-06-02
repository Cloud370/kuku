# Why kuku

kuku is a Rust SDK for agent execution where the runtime state stays in ordinary files.

## Core idea

- A `Session` is a directory.
- The event log is append-only.
- Project instructions, `Memory`, permissions, Agents, and Skills are file-backed.
- Hosts rebuild context from those files before each model call.

The result is a runtime you can inspect with normal tools. You can read the event log, diff a run, or commit the surrounding state to git.

## What kuku is

- An SDK that host apps build on.
- A runtime that persists agent facts to disk.
- A shared execution model for CLI, server, and other hosts.

## What kuku is not

| If you want | kuku is not that |
|-------------|-------------------|
| A hidden session store | State is files on disk. |
| A single chat app product | Hosts are separate apps built on the SDK. |
| A plugin-first core | The core loop stays small; extensions attach around it. |

## Why the file-native model matters

- State is inspectable without special tooling.
- Recovery uses persisted events instead of in-memory assumptions.
- Host apps can present the same runtime facts in different ways.
- Operational rules live near the project instead of inside one host.

Read [File-Native Model](file-native-model.md) next, then [Agent Loop](agent-loop.md). For maintainer-facing structure, see [Architecture Overview](../architecture/overview.md).
