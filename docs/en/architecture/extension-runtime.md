# Extension Runtime

Extensions attach around the core runtime through packages, hooks, Skills, and future external tool sources.

## Package model

Packages are discovered from user and project package directories under `.kuku/packages/` and `~/.kuku/packages/`.

A package can bundle:

- hook executables
- Skills
- MCP configuration for future external tool integration

The package manifest is the source of truth for hook registration.

## Hook model

Hooks run as external processes. kuku passes minimal structured input, reads stdout, and interprets the exit code.

This keeps the extension boundary:

- language-agnostic
- auditable
- isolated from the SDK process

## Runtime integration points

Implemented lifecycle hooks cover these points:

- `session.start`
- `session.end`
- `tool.pre_execute`
- `tool.post_execute`
- `model.pre_request`
- `model.post_response`

Hooks can add context, modify some inputs or outputs, or block operations at specific points. They do not replace the core session model.

## Permission boundary

Extensions do not bypass the permission gate. A package can influence behavior around the loop, but hard guards and runtime permission checks still apply.

## Skills inside packages

Package-bundled Skills are discovered by the same skill registry as standalone Skills. The runtime model stays the same; packaging only changes distribution and co-location with hooks.

## Forward path

The next major extension boundary is external tool sources such as MCP-backed tool providers. That work builds on the same runtime assumptions: stable tool schemas, runtime permissions, and file-backed session truth.

See [Host Apps](host-apps.md) for host boundaries and [Evolution](evolution.md) for the current implementation sequence.
