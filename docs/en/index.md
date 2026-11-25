# kuku Docs

kuku is a fast, low-memory, file-native agent runtime and Rust SDK. On a 371 MiB RAM ARM64 board, it can open and continue a 1,000+ turn conversation in the terminal while paging through older history. [Performance measurements](how-it-works/performance.md#low-memory-arm64-device)

**Before the first stable release, kuku supports only the current public APIs and Session format.** Updates may break SDK, CLI, HTTP, or streaming integrations and may make older Sessions unreadable.

## Sections

- [Start](start/index.md) — install kuku, run a first task, and configure models.
- [Guides](guides/index.md) — step-by-step workflows for tasks, Sessions, Memory, Agents, Skills, and the server.
- [How It Works](how-it-works/index.md) — the file-native model, Agent loop, Session lifecycle, permissions, and performance measurements.
- [Reference](reference/index.md) — CLI commands, config, environment variables, tools, file formats, SDK, and Server API.
- [Architecture](architecture/index.md) — crate boundaries, host apps, prompt assembly, and internal design.
- [Contributing](contributing/index.md) — development workflow, testing, code style, and releases.

## First Task

- [Start in the terminal](start/quickstart.md) to configure a model and send a message.
- [Run from a script](guides/run-a-task.md) to configure and submit a headless task, then read its result.
- [Embed the Rust SDK](reference/sdk.md#first-request) to open a Session and make a first request.

## Key Pages

- [Core Direction — Read First](architecture/core-direction.md) — concise target baseline for core development; not a claim of shipped behavior.
- [Performance](how-it-works/performance.md)
- [Session files and the Rust SDK](how-it-works/why-kuku.md)
- [Agent Loop](how-it-works/agent-loop.md)
- [File Layout](reference/file-layout.md)
- [Server API](reference/server-api.md)
- [Development](contributing/development.md)
