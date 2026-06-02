# Host Apps

Host apps present SDK facts to users. They do not replace the SDK runtime model.

## Repo layout

```text
crates/
|- kuku/
|- kuku-cli/
`- kuku-server/

apps/
|- kuku/
|- web/
`- tauri/  planned
```

## Responsibilities

### SDK

The SDK owns:

- session state and event persistence
- context rebuild
- provider adapters
- tool dispatch
- permission decisions
- wire event conversion

### Hosts

Hosts own:

- command parsing
- UI layout
- transport details
- permission prompts and other interaction surfaces
- presentation of session history and streaming output

## Current hosts

### CLI

`kuku-cli` implements command behavior for runs, session inspection, config operations, prompt asset inspection, and catalog views for Agents and Skills.

### Server

`kuku-server` runs a long-lived HTTP process and streams run events as NDJSON. It keeps active runs in memory, but durable state still comes from session files written by the SDK.

### Web

`apps/web` is a frontend SPA that talks to `kuku-server` over HTTP.

### Tauri

`apps/tauri` is planned as a desktop shell that embeds `kuku-server` rather than calling the SDK directly.

## Design rule

No host embeds another host. Each host calls the SDK directly or, in the Tauri case, embeds the server library as its transport layer.

See [Architecture Overview](overview.md) for the main split and [Extension Runtime](extension-runtime.md) for how packages and hooks attach around the runtime.
