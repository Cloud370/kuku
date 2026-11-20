# Architecture

Use this section for maintainer-facing design: crate boundaries, host structure, prompt assembly, and internal runtime contracts.

## Pages

- [Core Direction — Read First](core-direction.md) — concise target decisions, retained strengths, non-goals, and next usable result
- [Core Evolution](evolution.md) — delivery stages, implementation rationale, and acceptance conditions; progress belongs to the development workflow
- [Overview](overview.md)
- [Host Apps](host-apps.md)
- [Interactive clients — server, Rust TUI and WebUI](interactive-clients.md) — proposed client contract, delivery order and terminal testing
- [Web client](web-client.md) — browser workbench design: interaction model, token system, projection contract and delivery stages
- [Workspace execution and Session variables](workspace-execution.md) — portable scripts, host facts, named data and adoption criteria
- [Prompt Assembly](prompt-assembly.md)
- [Module Contracts](module-contracts.md)
- [Extension Runtime](extension-runtime.md)

## Start Here

For core work, read [Core Direction](core-direction.md), then only the relevant code and [Evolution](evolution.md) section. For current implementation structure, use [Overview](overview.md), [Host Apps](host-apps.md), or [Module Contracts](module-contracts.md). Target planning does not imply that the implementation has changed.
