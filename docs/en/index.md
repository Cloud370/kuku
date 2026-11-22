# kuku Docs

kuku is a file-native Rust Agent SDK in testing. Open `kuku` in your workspace to start a task in the terminal. Use the browser to return to the same Session. [Install](start/install.md) · [First task](start/quickstart.md) · [Run the server](guides/run-the-server.md)

An idle Linux server used about **16 MiB RSS**. The SDK reopens sealed Sessions from an inline journal checkpoint. [Performance measurements](how-it-works/performance.md)

## Sections

- [Start](start/index.md) - install kuku, run it once, and set up configuration.
- [Guides](guides/index.md) - complete common tasks with step-by-step flows.
- [How It Works](how-it-works/index.md) - understand the runtime model, Session flow, Memory, and permissions.
- [Reference](reference/index.md) - look up commands, config, environment variables, formats, APIs, and glossary terms.
- [Architecture](architecture/index.md) - inspect crate boundaries, host apps, prompt assembly, and internal design.
- [Contributing](contributing/index.md) - follow the development, testing, and release workflow.

## Recommended Path

1. [Install](start/install.md)
2. [Quickstart](start/quickstart.md)
3. [Configuration](start/configuration.md)
4. [Run a Task](guides/run-a-task.md)
5. [CLI Reference](reference/cli.md)

## Key Pages

- [Core Direction — Read First](architecture/core-direction.md) — concise target baseline for core development; not shipped behavior.
- [Why kuku](how-it-works/why-kuku.md)
- [Agent Loop](how-it-works/agent-loop.md)
- [File Layout](reference/file-layout.md)
- [Server API](reference/server-api.md)
- [Development](contributing/development.md)
