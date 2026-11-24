# Rust code style

Use the checks below for objective rules. Apply the defaults consistently in new code and follow a module's established pattern when it gives the same guarantees.

## Required checks

- Let rustfmt own layout. Run `cargo fmt --all -- --check` for a read-only check. Format the edited Rust files once after editing.
- Use the workspace lint settings. Clippy warnings fail CI. Check the affected crate locally with `cargo clippy -p PACKAGE --all-targets -- -D warnings`, adding the features that activate the edited paths. Use the integration selector for the full check. [Testing](testing.md)
- Put each unsafe operation in an explicit `unsafe` block, including inside an `unsafe fn`. Add a nearby `// SAFETY:` comment for blocks and implementations that states the preconditions and how the code satisfies them. Document the caller's obligations under `# Safety` on public unsafe APIs.
- Give each lint exception a `reason = "…"` at the narrowest applicable scope. Prefer `#[expect(...)]` when the lint is expected to fire across the checked configurations. Use `#[allow(...)]` for intentional exceptions whose applicability varies by feature, target or test binary.
- Review the affected platform and feature selections. Cargo's `--all-targets` includes library, binary, example and test targets for the selected platform.

## SDK and runtime contracts

- Use the narrowest visibility that serves the callers. Treat externally reachable items, including re-exports, as SDK surface. Document behavior that callers rely on, including errors, effects and cancellation where applicable. [Delivery and engineering choices](../architecture/core-direction.md#delivery-and-engineering-choices)
- Reject malformed or out-of-range external values with an error that identifies the operation and value. Use checked conversions for narrowing. Use `Result` for expected file, network and SDK failures. An internal `expect` should identify the invariant. A `debug_assert!` may check an internal invariant. Input validation, authorization and required effects must execute in release builds.
- For required filesystem operations, handle errors from the operation itself. An optional file may use a default on `NotFound`. Use atomic exclusive creation for paths that must be new. Keep path identity in `Path` or `PathBuf` and use a lossless representation for persisted identity. Lossy path display is suitable for human-readable labels.
- Keep credentials out of diagnostics. Reuse redacting representations and check changed diagnostic paths with a fake credential. Use the SDK's explicit workspace and settings. Set each child process's environment and working directory on its command. Inject test settings or use a separate process when a test needs environment changes.
- Make the owner, bound, cancellation and completion of effectful tasks explicit. End synchronous lock scopes before asynchronous I/O, and arrange cleanup and joining where the resource lives. Propagate errors that affect the result. Acknowledge a durable effect only after its required write succeeds. [Core direction](../architecture/core-direction.md#waiting-and-resource-lifetime)
- Use enums for closed states and make state transitions, authorization decisions and error mappings exhaustive. Handle unknown values explicitly at open protocol boundaries. Use distinct types for identifiers or units that callers could confuse. Mark custom handles that must be retained to take effect with `#[must_use]`.

## Everyday defaults

- Keep business logic together by responsibility. Use named functions for coherent steps and share the existing implementation when behavior matches. Consider splitting a long file at an ownership boundary. Use comments for invariants, platform requirements and decisions the code alone cannot express.
- Prefer `?` for propagation and `let ... else` for a guard with an early exit. Use `if let` for a local branch, `matches!` for a predicate and `match` when branches describe different outcomes. Use a Rust 2024 let chain when a binding and its condition describe one check. Use the standard library for well-defined operations such as UTF-8 byte truncation with `floor_char_boundary`.
- Group imports by standard library, external crates and local crate. Prefer explicit imports. A test module may import its parent's items with `use super::*`. Use names that describe the domain and consistent naming within a module.
- Place a unit test near the implementation and exercise public contracts through their actual entry points. Name assertions by adding context where a failure would be unclear. Reuse the existing fixtures and keep paid calls opt-in. [Testing](testing.md)

## Dependencies

Share versions of direct dependencies in `[workspace.dependencies]` when multiple workspace members use them. Keep each member's feature requirements explicit. Review manifest and lockfile changes together.

For a dependency change, use `cargo tree --locked --workspace --target all --duplicates` to find multiple versions and `cargo tree --locked --target all --invert NAME@VERSION` to trace a particular version. Compare the application's normal graph with `cargo tree --locked -p kuku-app --edges normal --duplicates` when assessing shipped code. Existing upstream or test-only versions are useful review evidence and can coexist. Check configuration and native behavior when changing a library that owns those boundaries.
