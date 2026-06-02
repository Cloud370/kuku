# Evolution

This page records the current direction of the architecture and the major implementation phases already completed or still planned.

## Stable direction

Several design choices are meant to stay stable:

- file-backed runtime facts
- append-only event persistence
- host apps separated from SDK internals
- Skills as native instruction loading
- extensions as external boundaries, not core runtime features

## Implemented path so far

| Phase | What | Layer | Status |
|-------|------|-------|--------|
| 1 | Skills and registry loading | SDK | implemented |
| 2 | wire-facing `UiEvent` shape | SDK | implemented |
| 3 | NDJSON wire serialization | SDK | implemented |
| 4 | HTTP server host | host | implemented |
| 5 | web host | host | implemented |
| 6 | package and hook runtime | SDK | implemented |

## Planned path

| Next | Purpose |
|------|---------|
| MCP-backed external tool sources | add non-core tool providers through the extension boundary |
| Tauri host | desktop shell built around the server runtime |

## Design pressure to watch

- keep provider logic isolated from session and permission logic
- keep prompt assembly stable as more runtime notices appear
- keep extension points outside the core loop unless they are fundamental runtime behavior
- keep public mental-model docs separate from maintainer internals

If a future change mainly affects user-visible behavior, document it in `how-it-works/`. If it mainly affects crate boundaries or internal ownership, document it here in `architecture/`.
