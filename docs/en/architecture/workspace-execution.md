# Workspace execution and Session variables

## Product commitments

kuku should let a model complete real engineering work on a stock Windows installation without requiring Python, Node.js, Git Bash or proficiency in PowerShell. Linux and macOS are first-class hosts. Keep the Rust SDK small and usable on its own. Idle Agents, ordinary conversation and single file operations should incur no script-worker cost. After a large task, memory should fall back toward the idle baseline. Measure on both constrained and well-equipped execution hosts.

Users, models and SDK callers should agree about what was requested, what ran and which results exist. Every tool should produce useful model-facing evidence and machine-readable facts for hosts. A TUI can summarize and expand those facts. Future clients can consume the same SDK contract. Existing internal APIs and demo clients may be replaced when they impede these goals.

## Proposed execution surface

Embed a pinned QuickJS engine in the distributed kuku binary and activate a worker on demand. Accept a JavaScript function body with top-level `await` and a return value. Standard JavaScript supplies variables, template literals, loops, conditionals, JSON and promises. Publish the available host methods and their actual signatures. A stock installation should run the same workspace script across all three OSes. Python's familiar standard library is a real advantage for tasks such as quoted multiline CSV. Supply a small, correct parser when a recurring task warrants one. A separate Python distribution, Node compatibility layer and global template-expansion pass carry substantial costs for this use case.

| Work | Proposed entry point and ownership |
|---|---|
| Inspect, search or precisely change one file | Keep concise direct tools. Scripts may compose the same authorized actions. Give actions with equal semantics the same name and one Rust implementation. Render line numbers and decoration in the view. |
| Batch or dependent workspace operations | One script may find, search, read, edit, write, stat and remove files. Keep intermediate values local. Teach a complete read → compute → deliver example. Models should inspect unknown formats and otherwise finish a specified transformation in one call. |
| Run an installed program | Give the model `run_command({program, args?, cwd?, background?, title?})`. Pass arguments literally. Resolve an omitted `cwd` to the Agent workspace. A top-level `title` explains the purpose of a command or script to the user. Direct tools and `run_js` share the Rust process executor. Embedding hosts can set a process deadline. Tell the model who controls the deadline in both descriptions and when it supplies an unsupported `timeout` argument. |
| Run a shell script or use shell syntax | Launch the required interpreter explicitly with literal arguments. Embedding hosts can choose a typed shell invocation. Authorize and record the actual invocation, including its interpreter and working directory. |
| Delegate, ask a user or wait for a reply | Session owns durable receipts, identities and waits. A child Agent may run scripts within its granted tools. A no-tools child can only reply. Consider an optional first task on creation, separate from standing instructions. A targeted wait should replace model polling. |

Keep other tools directly discoverable. Expose an extension to JS when a real task benefits from composing its operations. A general string-based tool dispatcher and duplicate descriptions of every host tool would increase the model's search space. Let reusable, verified procedures graduate to ordinary project scripts.

## Host facts and model evidence

Start from the existing [module contracts](module-contracts.md) and [retained output](../reference/tools.md). Define common outcomes, action identity and output references in the public SDK. Add typed facts specific to the action, including exit status, observed file changes or a reported test result. Keep capture completeness, partial execution and unknown results explicit. Native tools and scripts share the authorization, execution and reporting boundaries. The TUI derives cards and drill-down from actual host facts. Arbitrary programs can have additional effects the host cannot identify.

A test runner with a recognized report can yield counts, the first failing case, a bounded excerpt and a reference. An unfamiliar runner can yield an exit status and a useful excerpt. Never infer a passing suite from a successful script return. Keep model evidence bounded but sufficient for the next judgment. Hosts must be able to handle results without parsing model prose. Preserve the exact view already accepted into model context.

A writer that supplied the entire contents of a file and received confirmation of its successful write knows that version. It should be able to edit or overwrite that same version without reading its own output back. Scope this evidence to the originating actor and its effective history. A focused edit can use exact text supplied by the caller when it matches one location in the current file. A current partial read can distinguish repeated occurrences. A partial read or edit establishes only the range observed for operations requiring whole-file knowledge. A deterministic script can transform a fully read file without sending the whole file to the model. The host must still authorize the action and verify the current version at dispatch.

A top-level command or workspace script starts once. The observation window defaults to ten seconds for foreground calls and one second for explicit background calls. A call still running at the boundary returns a Job handle. Promotion preserves the execution and its retained output. A timer alone does not schedule model inference. New input can steer the Agent while work continues. An awaited script command always resolves to its final typed outcome. Host I/O wait is separate from the JavaScript execution budget. The owning Session delivers completion once and supports inspection and cancellation.

Schedule workspace reads together within a model response. Dispatch workspace mutations, arbitrary programs and scripts in call order with respect to reads and other workspace mutations. A read queued after a blocked mutation waits for it. Calls outside the workspace can continue independently, subject to authorization and capacity. Once a command or script returns a Job handle, the caller can continue while the Job runs. Each call retains its own result and authorization.

Validate the complete tool batch before admitting any call. If a complete model response declares tool use with malformed argument JSON, the Session can regenerate once per turn under its retry setting and request budget. Keep the rejected response out of subsequent model input. Persist the cause and recovery so users can distinguish it from a failed tool.

## Proposed command output path

Keep one execution path across direct tools, JavaScript and Jobs. The tool catalog and Prompt explain how to choose a tool. The executor prepares and authorizes the final invocation. The capture writer owns stdout, stderr, quotas and the retained reference. Rust observers derive evidence from this capture. The model gets a bounded view, the clients use the same structured facts and output reference, and context admission records the exact view sent.

Use complete short output when it fits the model preview budget. Start with an additional short-output limit of 2 KiB and 40 lines, then calibrate against real workloads. A long result provides useful evidence, the observed data shape and a retained reference. Label whether the shape comes from a prefix or a validated format. Show representative fields or columns and a short extraction hint for the methods actually available to the Agent. Give failures diagnostic space first. Distinguish the process outcome, capture completeness and the presentation limit. Keep the same presentation policy for foreground results and automatic Job delivery.

The default Prompt should favor `run_js` for deterministic processing that would otherwise require multiple model Steps or bring intermediate data into model context. Give the model examples grounded in the exposed methods and actual response fields. Keep direct tools the natural choice for one file operation and direct native invocation for programs. Measure script selection on tasks that can usefully compose work, along with first-attempt correctness and total model requests.

`read_output` is the raw range reader. Add a JavaScript-callable literal search over the same authorized capture, initially exposed as `tools.search_output`. Rust scans with bounded memory and checks cancellation. Return a small set of matches with stream and original byte offsets, plus continuation and capture-completeness information. Keep intermediate parsing in the local script when the task needs custom logic. The TUI and model can read the same range independently.

### Built-in rule format

Use an explicit subset of RTK's TOML rule format for line-level filtering. Share `[filters.<name>]`, `description`, `match_command`, `strip_ansi`, the mutually exclusive `strip_lines_matching` and `keep_lines_matching`, `head_lines`, `tail_lines`, `max_lines`, and `[[tests.<name>]]`. Match RTK's processing order for these fields. Compile rules once, reject unsupported fields with a diagnostic, and pin the upstream revision when borrowing a rule and its fixtures. [RTK filter format and order](https://github.com/rtk-ai/rtk/blob/b8cb15167268b7a27519b84f79e53c4cb742bafe/src/core/toml_filter.rs#L600)

Select a rule using the prepared native program and argument boundaries. Evaluate `match_command` on a deterministic rendering of that invocation for rule matching only. Treat output modes and reporters as part of eligibility. Preserve the authorized command and arguments. Use the generic preview when selection is ambiguous or the captured format has no verified rule.

Use TOML for predictable noise lines and a Rust parser for multiline diagnostics and test reports. Extend the current streaming report collector and use one view builder for both. A view records its kind and the selected rule identity. The process outcome stays authoritative. Label summaries and preserve source ranges for actionable evidence. Store ranges only for retained bytes, including stream identity when stdout and stderr interleave. Keep the generic view available when a formatter fails, loses decisive evidence or expands the result.

| Initial rule family | Evidence to keep |
|---|---|
| `cargo build`, `cargo check`, `cargo clippy` | Outcome, diagnostic blocks, source locations and warnings |
| `cargo test` | Suite counts, failing tests and relevant assertions |
| `pytest` | Observed outcome counts, failure names and locations |
| `go test` | Affected packages, failed tests and diagnostic excerpts |
| `tsc` | Source locations, error codes and diagnostic messages |
| `vitest`, `jest` | Suite counts, failed cases and relevant assertions for recognized reporters |
| `npm install`, `npm ci`, `pnpm install` | Resolution and lifecycle errors, dependency conflicts and security warnings |
| `git status` | File status groups and conflicts for recognized long results |

Use the actual captured format in every family. A rule borrowed from RTK may depend on additional flags or a different reporter, so port the fixtures and verify the same mode before enabling it. Output whose body is the requested evidence, such as a diff or a search result, uses the generic preview and raw range reader.

### Delivery order

1. Stabilize the native command interface and generic preview. Cover the one-second Job observation and result delivery in the same public execution workflow.
2. Add Rust-backed literal search through the existing retained-output reader and JS host boundary.
3. Land the shared rule format with one full Cargo slice, including diagnostics, source ranges and replay tests. Add the remaining rule families in independently verified batches before claiming first-release coverage.
4. Compare the full candidate and the generic-preview baseline through the SDK, installed TUI and representative tasks. Use the recorded evidence to select the next improvement. [Command output acceptance](../contributing/testing.md#command-output-and-long-task-acceptance)

## Output and named data

Use one bounded reader for direct tools and JS. Offer line iteration and byte chunks for scripts. Preserve stream identity, actual offsets, decoding boundaries, raw bytes and truncation. JSON and regular expressions are native script operations. Multiline CSV needs a record parser. Small structured results may return inline. Large derived data should stream to a fresh Session output and return its reference after it is saved. Original logs remain independently readable. Write a workspace file when the user wants a deliverable.

A URL download returns a Session output reference that the same Agent can inspect with the output reader. Keep the downloaded bytes out of the project tree unless the user requests a file.

Image inspection uses the selected model and protocol's native tool-result media. Authorize the normalized workspace path and save a bounded snapshot in Session output storage. Context admits the image type and immutable reference. Request preparation loads eligible references from the current and previous turn within the remaining wire budget, and the adapter renders them for its protocol. Older results contribute their text. Retention pins the recent snapshots and applies the regular expiry policy to older ones. Verify replay after the workspace file changes and after close and reopen, and check refusal before reading a protected or denied path. See [tool reference](../reference/tools.md).

| Data | Lifetime and visibility |
|---|---|
| Script locals, functions and promises | One invocation. Release the heap when the worker is disposed. |
| Ordinary tool output | Each result has a traceable receipt and bounded retention. Models can compare two recent builds without predicting which one should be named. If bytes have expired, report that fact while preserving the execution outcome. |
| Explicitly named Session value | Bind a readable name to a specific value or existing output. Persist across Turns, disconnect, close and reopen. Load metadata first, content on demand. Default visibility is the owning Agent. Share a selected reference with another Agent. |
| Workspace deliverable | An ordinary user file with its own lifecycle. The same workspace may host independent Sessions. |

Let users bind a paste or attachment **before** it enters model context. A model may bind an identified result by name and source without copying its text. An explicit selection such as a TUI mention carries the resolved reference. Discover relevant names on demand. Similar names must not silently resolve to a different value. Repeating a binding to the same source is idempotent. A different source under an existing name needs an explicit update. Persist the new content before switching the binding. Already dispatched work or delegated inputs continue to refer to the value originally selected. A live path and a saved baseline have deliberately different semantics.

Bound the number of names and the retained bytes per Session. Rebinding an alias should not duplicate its target. Reclaim ordinary outputs after their promised retention when no named value, in-flight operation or handoff depends on them. When space is insufficient, preserve existing baselines and report the failed update or truncated capture. Deleting a Session leaves user workspace files intact. Project-wide variables, persistent JS heaps and a standalone artifact service have no established first-slice use.

Credential aliases are an optional convenience to reduce repeated pasting and accidental display. Prefer existing configured credentials. Keep secrets out of examples and avoid expanding an alias into a prompt or persisted tool arguments. Giving an external program a credential gives that program access to its value. A general secret manager or isolation guarantee is outside this proposal.

## Scheduling, interruption and resource ownership

Keep tool previews and reader pages small enough for the model to inspect. Stream the body to Session storage under the host's per-capture and total disk budgets. Report an exhausted budget or a write failure at the operation that encountered it. Model generation limits are configured independently.

Make host I/O genuinely asynchronous. Independent reads may use `Promise.all`. Dependent edits use `await` on their predecessor. Partial-success workflows may use `Promise.allSettled`. Batch large inputs so the JS heap does not retain every file. Cap heap, stack, elapsed time, host buffers, queued and active work, captured bytes and retained disk usage. Share execution budgets across Agents on the **execution host**. An attached phone affects client layout and rendering, not a desktop server's execution budget. Start with configurable limits and measure before adding adaptive scheduling or a warm worker pool.

Check the actual Agent, action, final arguments and target when dispatching each side effect. Program execution is its own permission since a program can modify files outside file-tool controls. Reuse process-tree cleanup and output capture. On interruption, stop new dispatch, settle owned work and report completed actions. A new user instruction takes effect at a safe boundary. A TUI may show “Stopped · 12 of 40 files changed” when those counts are known.

A deferred action can retain its prepared arguments, content reference, version conditions and receipt. The host may release the worker while waiting. On approval, revalidate and execute the matching action once, then deliver its outcome for continued work. A discarded VM cannot resume an arbitrary JavaScript continuation. After interruption or restart, never silently rerun a program with unknown side effects. Session owns durable waits and recovery as described in [Core Direction](core-direction.md).

For long work, make elapsed execution and observed progress available at existing tool and Job boundaries. Keep process deadlines separate from observation windows and measure elapsed time using a monotonic clock. A quiet interval is evidence of silence. A stop decision can use a documented deadline, a definitive outcome or a concrete blocker. Deliver terminal outcomes promptly and let independent work continue. Coalesce progress with work the Agent is ready to handle. Inference should follow useful evidence or an explicit input.

## Evidence needed for adoption

The implementation choice remains a proposal. A standalone native Windows embed demonstrates that a small statically linked host can evaluate source without Node or Python. Its executable size is not the release-size delta or resident memory of kuku. Five one-shot DeepSeek Flash requests at explicit low effort with only one tool available explored script familiarity and host-interface mistakes. They cannot establish tool selection, general model superiority or end-to-end completion. The local evidence directory is `benchmarks/acceptance/windows-js-capability/` in the originating checkout.

1. Deliver a vertical slice using the public SDK and installed TUI. Run an installed program, analyze its retained output in JS, expose structured facts and bounded model evidence, and reuse a named result after reopening the Session.
2. On native Windows, Linux and macOS, verify equal fixture outputs, Unicode and literal arguments including spaces, quotes and ampersands, missing executables, shell-specific entry points, encodings, timeout, cancellation and cleanup. Include stock Windows without installed scripting runtimes.
3. With the **full** eligible tool set, use DeepSeek Flash at high reasoning as the primary acceptance configuration and a lower setting to expose usability problems. Compare direct and script paths on a single edit and a large-log investigation. Measure correctness, calls, context bytes, tokens and elapsed time. Include ambiguous material, duplicate names, missing references and recovery from a failed first action.
4. Measure idle incremental memory and CPU, cold start, peak memory, disk growth and memory after completion on constrained and normal hosts. Run repeated builds, retain one baseline, resume it and verify the workspace stays clean. Include interrupted approval, partial writes, Agent handoff without polling and reconnect on an active job.

Set numerical limits, public type names, the initial JS binding list, worker reuse and further extension exposure using this evidence. Keep script glue out of the core Session state machine. Separate the design review from implementation and from acceptance of the preceding interactive-client branch.
