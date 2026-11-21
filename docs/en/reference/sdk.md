# SDK

`kuku::Session` is the embedding entry point. [`session_task.rs`](../../../crates/kuku/examples/session_task.rs) is an ordinary Rust caller. The product server uses the same SDK and exposes it to terminal, script and browser clients through the [Server API](server-api.md).

## Operations

| Operation | Result |
|---|---|
| `Session::open(SessionOptions).await` | Acquire writer ownership and reconstruct facts without automatic activation |
| `Session::open_for_creation(options, &input, &config).await` | Claim an explicitly named Session for a first-send intent, or reopen that same claim after interrupted creation |
| `create_agent(command_id, address, AgentConfig)` | Durable idempotent creation receipt |
| `configure_agent(command_id, address, AgentConfig)` | Agent-local defaults for the next safe boundary |
| `send(Input)` / `advance(address)` | Durable delivery and explicit activation |
| `send_configured(Input, AgentConfig)` | Commit delivery and configuration together with a correlation that identifies the configuration intent |
| `agent(address)` / `agents()` | Query derived snapshots without consuming output |
| `agent_view(address)` | Selected Agent snapshot, queued inputs and a matching committed cursor |
| `agent_selection(address)` | Inspect effective model settings, capabilities and final tool schemas without dispatch |
| `receipt(command_id)` / `input_receipt(&Input)` | Query and verify an earlier acknowledgement without resuming work |
| `input_status(address, input_command_id)` | Query whether an input is queued, accepted or withdrawn |
| `withdraw_input(command_id, address, input_command_id)` | Withdraw an input if it remains queued and return its status |
| `advance_command(command_id, address)` | Idempotent explicit advancement |
| `reply_permission_receipt(...)` / `reply_question_receipt(...)` | Verify an earlier reply acknowledgement without resuming work |
| `events(after, limit)` / `observe(after)` | Bounded pages and reconnectable observation |
| `read_output(output_id, &OutputRead)` | Bounded range, literal search, or incremental output |
| `reply_permission(command_id, address, request_id, allow)` | Exact pending-action reply |
| `reply_question(command_id, address, request_id, text)` | Exact question reply delivered as correlated input |
| `cancel(address).await` / `close().await` | Await owned cleanup and durable settlement |
| `cancel_execution(command_id, address, execution_id).await` | Cancel the particular observed execution and await its owned cleanup |
| `Session::read_agents(events_path)` | Offline recovery snapshots without runtime resources |
| `Session::read_view(events_path, address)` / `Session::read_receipt(events_path, command_id)` | Read the selected Agent view or command receipt from a closed Session |

The lower-level `Query` `Run` exposes host events through the non-exhaustive `UiEvent` and `ToolEvent` enums; keep a fallback match arm for each. Both enums include `ModelRequest` and `ModelRecovery` variants; `ModelRequest` carries the request ID and ordinal, while `ModelRecovery` carries request IDs and bounded retry information. See [Agent loop](../how-it-works/agent-loop.md) for their behavior.

`SessionOptions::new(workspace, data_dir, config)` accepts named-provider configuration. Options add prompt assets, resource limits, and a host `ToolAuthorizer`. `AgentConfig` selects preset, tier, named connection, model, reasoning, service tier, output tokens, instructions, directory, and native skill defaults. Connection selection is independent of wire protocol. An explicit model overrides the selected tier’s model while retaining its other defaults. `AgentConfig.tools` overrides the preset’s final tool selection; collaboration tools are not added to a read-only preset. Preset instructions, tier and tool defaults are resolved and stored when creating or configuring the Agent. Later preset-file changes do not change those retained defaults. Lifecycle and receipt semantics live in [Sessions](../how-it-works/sessions.md).

`Input.timing` distinguishes `InputTiming::NextStep`, the default, from `InputTiming::NextTurn`. `AgentSnapshot.execution_id` identifies the current execution. `Observation::Live` carries its source Agent, execution identity, Turn and latest committed cursor when the update was published. Use `agent_view` to start observation at the cursor that matches the selected Agent state.

`AgentSnapshot.config` is the requested configuration, and `active_config` records the configuration used by active or waiting execution. Model and reasoning settings can change at the next unprepared model request. Workspace, behavior and tool settings take effect on the next Turn.

When exposing retryable creation over a transport, `open_for_creation` checks the initial input, configuration and optional Prompt directory against the persistent claim. The host still creates the Agent and submits its input using stable command identities. Use `verify_creation(&input, &config, prompts_dir)` if the claimed Session is already open.

`Session::read_agents` reconstructs offline recovery state from the journal and maps an unterminated `Running` state to `Interrupted`, even while another runtime is active. Use an open Session's `agent` or `agents` methods for its current live state.

## Host adapters

Set `SessionOptions.model_adapter` to an `Arc<dyn ModelAdapter>` to supply a model transport, and append `Arc<dyn ToolAdapter>` values to `SessionOptions.tools` for additional tools. The [external-caller tests](../../../crates/kuku/tests/adapter_runtime.rs) exercise both through the public SDK with real file actions, host denial, retained output and awaited cancellation cleanup.

A `ModelAdapter` receives the frozen `ModelRequest`: selected connection, protocol, endpoint, model, native settings, correlation and the exact prepared body, without authentication headers. It returns a `ModelStream` of `ModelEvent` values. Configuration selection and `agent_selection` do not resolve environment credentials; the built-in HTTP transport resolves them immediately before sending. Custom transports own authentication. The SDK retains the same preparation barrier, request quota, model capacity, response validation, usage and journal path. Dropping the adapter's pending future or returned stream must release its transport resources.

Tool call IDs must be unique across all model steps within one Agent Turn, including after resumption. A separate Agent or Turn may reuse an ID.

A `ToolAdapter` supplies an immutable `ToolSpec` at open and executes the final `ToolAuthorization` with a `ToolCancellation` signal. Built-in names cannot be replaced. Presets and `AgentConfig.tools` select from the combined available definitions. Every invocation passes the existing host authorizer and durable preparation/dispatch boundary. Declared writes and external actions join the Agent’s ordered workspace action group; declared reads stay outside that group and may overlap other tools. The tool must clean up owned work before returning, including after cancellation; the SDK waits for that return. A known incomplete cleanup is returned as `ToolOutput.cleanup_incomplete` and makes `cancel` or `close` fail.

Tool content uses the command output format and public `read_output` fallback. The first model view includes a complete reference and truncation flags and is bounded by `ToolSpec.max_result_bytes` and the runtime preview limit. Oversized structured data is retained in that output instead of the journal. Returned buffers are allocated by the host adapter; the SDK's retained capture and model view have separate limits.

## Workspace execution

Models call [`run_command` or `run_js`](tools.md#run-a-native-program) using the Session's selected tools. An embedding host reads the `structured` field of `kuku::event::EventPayload::ToolResult` via `Session::events` or `Session::observe`. Decode a command outcome with `kuku::execution::CommandExecutionFacts::from_structured`. `invocation` preserves the program and argument boundaries or the selected interpreter and its command text. `started`, `termination`, `exit_code`, and `signal` describe the process outcome as observed by the host. `capture_complete`, `cleanup_incomplete`, and `output_ref` describe retained evidence and cleanup. As a trusted host, use `Session::read_output` with `output_ref.id` to inspect a bounded page and `OutputSegment::as_bytes()` to read the original bytes. `reported_tests` contains counts observed in program output. Arbitrary external file effects may remain unknown.

When the tool returns a [Session Job](tools.md#session-jobs), call `Session::jobs(agent)` to list recent and running Jobs or `Session::job(agent, job_id)` to inspect one. Use `Session::wait_job(agent, job_id).await` to await its terminal result or `Session::cancel_job(agent, job_id).await` to request cancellation and await cleanup. Job states are `running`, `completed`, `failed`, `cancelled`, and `interrupted`. The terminal `JobSnapshot.result` contains the action's structured outcome. Once the command's capture is finalized, its `output_id` uses the same output reader as other captures and its `output_ref` gives the captured byte range.

`RuntimeLimits.job_foreground_wait_ms` controls the foreground observation window for top-level commands and scripts.

A `run_js` result has `kind: "workspace_script_result"`. Decode it with `kuku::WorkspaceScriptFacts::from_structured`. The result contains the script's return `value` and its observed host `actions`. Each action records its ID, tool, status, summary, truncation, and bounded result. An `unknown` action status means the host could not confirm its final outcome. At the script level, `timed_out`, `cleanup_incomplete`, and `not_dispatched` help reconcile partial work, including after a script fails. See [script methods](tools.md#workspace-scripts).

For the journal's nested dispatch and outcome facts, decode `workspace.action` events with `kuku::execution::WorkspaceActionReceipt::from_event`. Correlate by `action_id` within the parent `tool_call_id`. A dispatch record without a matching outcome leaves the effect unknown.

`kuku::execution::FileMutationFacts` describes a write, exact edit, or removal. The facts include the path, observed before and after hashes where available, byte counts, and `commit_state`. A `published_unconfirmed` state means the file change occurred and final persistence could not be confirmed. Inspect the file before retrying.

`RuntimeLimits.workspace_script` bounds each invocation's memory, JavaScript execution time, host requests, operation queue, and result size. Time spent awaiting a host action does not spend the JavaScript execution budget. To share capacity across Sessions, set `SessionOptions.workspace_host` to clones of a `WorkspaceExecutionHost`. The process-wide default allows two script workers and eight concurrent workspace operations. Script host actions and direct workspace file tools, commands, and model-facing output reads use the same operation pool. `WorkspaceExecutionHost::metrics` gives active, peak, and started worker counts without starting a worker.

## Named values

Call `Session::save_named_value(agent, name, source, replace).await` with a `kuku::session::NamedValueSource` of `Text`, `Json`, `Bytes`, or `Output`. `Output` binds an existing capture by ID without copying it. `Session::named_value` resolves one name. `Session::named_values` lists its Agent's bindings without reading payloads. For a closed Session, `Session::read_named_values(events_path, agent)` returns the same metadata without opening a writer.

Use `Session::read_agent_output(agent, output_id, &OutputRead)` to enforce the Agent's output access, and `Session::read_saved_agent_output` for the equivalent read from a closed Session. Submit resolved `OutputReference` values with `Session::send_with_values` or `Session::send_configured_with_values` to bind them to the input. [Naming and sharing semantics](tools.md#named-session-values)

## Calling program

```bash
cargo run -p kuku --example session_task -- \
  --workspace /path/to/work --prompt-file /path/to/task.txt \
  --config /path/to/config.toml --data-dir /path/to/session-data \
  --connection deepseek --reasoning-effort low \
  --output /path/to/result.json
```

Use `--session-id` to reopen and `--command-id` to retry delivery after losing a receipt. Results include durable model, tool, and Turn counts, the journal path and cursor, provider settings and reports, limits, duration, and cleanup. `runtime_completed` means execution settled. A benchmark still needs its actual judge. `--max-requests N` optionally caps attempts across this open runtime. The example reports `null` for `max_requests` when this option is omitted. `--max-agents` sets the retained Agent limit. `--max-context-tokens` caps the configured context window in tokens. `--max-request-bytes` independently caps serialized request bytes and inherits the SDK default when omitted.

## Limits and accounting

The default limits allow 64 Agents, 128 pending inputs per Agent, 65,536 input bytes, 512 KiB per prepared request, four simultaneous model streams, eight tools per Agent, 8 MiB per retained output, an 8 KiB first preview, and 128 transient observations. Output previews require at least 1 KiB for a complete reference and flags.

`RuntimeLimits.max_requests_per_open` optionally caps total attempts across all Agent Turns and resets on reopen. `RuntimeLimits.max_output_tokens_per_request` optionally sets a host-wide ceiling on each response. Both default to `None`. An explicit Agent output setting overrides the tier setting. The host ceiling applies to every Agent. In Rust, `TierConfig.max_output_tokens`, `AgentSelection.max_output_tokens`, and `ModelSelection.max_output_tokens` are `Option<u32>`. An absent effective value delegates to the provider on protocols that allow it. The preparation record contains the value used for that request. `session_task --max-output-tokens N` sets both the host ceiling and the primary Agent's setting. See [model configuration](config.md#modelname).

`RuntimeLimits.retry_transient_provider_errors` defaults to true. The SDK permits up to six total attempts (five retries), with one 2/4/8/16/32-second exponential schedule and 0–25% jitter for retryable provider failures. A valid `Retry-After` of at most 30 seconds can lengthen a wait; missing, invalid, and longer hints do not disable retry, and hints over 30 seconds are ignored. Explicit authentication, permission, quota, context, request, and invalid-response failures are not retried. The SDK may retry an error before the first stream content; once output has begun, a retryable failure ends the attempt to avoid repeating emitted or persisted effects. Each retry reuses the prepared request and counts only when dispatched toward the caller's request budget. Set the option to false or pass `session_task --no-provider-retry` to disable retries. The example reports five permitted retries under `limits.automatic_retries` (zero when disabled).

Journal records are capped at 1 MiB. Pages contain at most 256 records and stop at a 1 MiB target, with one-record overrun below 2 MiB. Output reads return at most 80,000 bytes. The retained-output quota includes capture framing and metadata. When capture hits its quota, the runtime continues draining the pipes and marks the retained result incomplete. The default Session budget is 128 MiB of retained data and 64 named bindings. Unreferenced ordinary outputs become eligible for reclamation after 24 hours. Set `max_retained_output_bytes`, `max_named_values`, and `output_retention_secs` in `RuntimeLimits` to change these bounds.

`UsageTotals` retains absent counters as `None`; `unreported_requests` identifies incomplete input/output reporting. Input totals include cached input, so cache counters are subsets. Raw optional provider reports remain separate facts. Known incomplete cleanup makes `cancel` or `close` fail.

## Controlled discussion and timer

[`session_orchestrate.rs`](../../../crates/kuku/examples/session_orchestrate.rs) is a small host for two independent Agents in one Session. It admits exactly three rounds: a writer draft, a reviewer response using the selected writer result, and a writer continuation after closing, reopening, and waiting for a host timer. The reviewer receives its own instructions and the selected result; the writer retains its own history across reopen.

Provide the task, reviewer instructions, and continuation instructions in three text files. For example, ask the writer to inspect and repair a small program and run its tests, ask the reviewer to inspect the shared files, then ask the writer to apply the selected review and check the result. Use named connections from your [configuration](config.md), with credentials supplied through environment references.

```bash
cargo run -p kuku --example session_orchestrate -- \
  --workspace /path/to/work --config /path/to/config.toml \
  --prompt-file /path/to/draft.txt \
  --review-prompt-file /path/to/review.txt \
  --continue-prompt-file /path/to/continue.txt \
  --writer-connection direct --writer-tier balanced \
  --reviewer-connection openrouter-openai-responses --reviewer-tier light \
  --max-requests 16 --timer-ms 250 \
  --max-output-tokens 32768 --max-context-tokens 131072 \
  --data-dir /path/to/session-data --output /path/to/result.json
```

The host sends the first two inputs with `Activation::Enqueue`, explicitly advances each recipient, and delivers the timer input as `InputSource::External("timer")` with a correlation. Every delivery has a stable command ID and verifies an identical retry's receipt. The example owns two Agents, disables model-created Agents and Skills, and caps requests separately for each of its two opens. Reopening renews that host-selected request allowance. Output and context default to 32,768 and 131,072 tokens. `--max-request-bytes` inherits the SDK's request byte limit when omitted.

The host owns the timer lifetime and starts it after reopen; the Session persists the delivered input. Observers are drained after execution settles, so subscription consumption cannot drive the work. JSON results retain per-round receipts and snapshots, provider and tool facts, bounded retained command output, dormant request counters, and close/reopen evidence. `runtime_completed` requires all three rounds to settle idle with no pending input and a successful durable close; verify task artifacts separately. A waiting Agent is reported as incomplete.
