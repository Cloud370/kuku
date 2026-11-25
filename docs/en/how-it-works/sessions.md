# Sessions

A `Session` owns the persistent journal, Agents, shared writer, retained command outputs, and live execution state for one workspace group. Embed the Rust SDK directly without a daemon or CLI subprocess. See [SDK reference](../reference/sdk.md) for the exact API.

## Identity and ownership

Agents have flat, stable Session-local addresses (`main`, `reviewer`, `agent-3`). Once created, an address is never reassigned. Every new Agent starts with a clean context. A preset can supply its instructions, exact model pair, and tools. Explicit overrides apply only to that Agent.

## Delivery, scheduling, and observation

Creation, configuration, input delivery, replies, and targeted cancellation all take a caller-supplied `command_id`. Repeating the same ID and payload returns the original durable receipt; reusing an ID with different payload fails. Delivery, model admission, and Turn completion are recorded as separate facts.

- **Activation**: `Activation::Wake` schedules advancement; `Activation::Enqueue` stores the input until the host advances explicitly.
- **Timing**: Running Agents admit eligible inputs in receipt order at safe boundaries. `InputTiming::NextStep` can enter before the next model request in the current Turn (once the in-flight response and dispatched tools settle), or in the next Turn if the current Turn finishes first. `InputTiming::NextTurn` always waits for the next Turn, even if a later `NextStep` input gets admitted earlier.
- **Retries**: Technical retries reuse the prepared request and apply only to recognized transient provider errors *before* stream output starts. Once output begins, no automatic retry happens.
- **Turn progression**: By default a Turn runs tool Steps until the model finishes, pauses/stops, or hits a configured resource limit.
- **Concurrency & cancellation**: Each Agent has at most one active execution chain, while distinct Agents run concurrently. Targeted cancellation references the observed execution ID and pauses queued inputs (resume by advancing or sending a waking input). Targeted interruption instead requests continuation of the existing queue after successful cleanup, without retrying the interrupted Turn. Queued inputs can be withdrawn by receipt ID; if withdrawal races with admission, it reports what actually happened.
- **Observation**: Execution is decoupled from observers. `agent_view` returns a snapshot, queued inputs, and committed cursor; `events(after, limit)` pages through committed records; `observe(after)` combines a resumable durable cursor with bounded live deltas keyed by Agent and execution ID. Slow observers get a lag notice and can always fall back to durable reads.

## Background Jobs

Long-running commands or workspace scripts can detach into a **Session Job**. A Job belongs to the Agent that started it, keeps running after the Turn finishes or clients disconnect (still using host resources), and honors any explicit process deadline. The foreground observation window decides when a tool returns a Job handle instead of blocking. [Tool arguments and Job handles](../reference/tools.md#session-jobs)

Hitting the observation window does not trigger a model request by itself. When the Job ends, the Session records its terminal state (non-zero exit or signal = failed) and delivers a bounded completion receipt plus any retained output ID to the owning Agent's inbox. The Agent picks it up at the next safe boundary without polling; clients render it as runtime feedback (not as a human rewind target) and can inspect or cancel live Jobs while the Session is open. [Terminal controls](../reference/cli.md#terminal-interface) · [SDK methods](../reference/sdk.md#workspace-execution)

- A targeted stop of an Agent's current execution leaves detached Jobs running.
- `Session::cancel(agent)` stops that Agent **and** cancels its Jobs.
- Closing a Session cancels all live Jobs and waits for cleanup and durable writes.
- If the process dies before a Job settles, reopening marks it `interrupted` — check actual side effects before re-running.

## Storage and journal sealing

Each [Session directory](../reference/file-layout.md#session-layout) holds one authoritative `events.jsonl` and its retained outputs. A single shared writer holds an OS file lock and appends synchronized records. Opening repairs any truncated tail line. Sealed prefixes carry an inline runtime checkpoint so reopening restores state directly and replays only the uncompressed tail; changing the model request byte limit keeps the checkpoint usable (startup context rebuilds lazily at the active limit when needed). If a checkpoint is unusable, kuku falls back to full replay. Packed frames are integrity-checked on read. [Measured reopen time and memory](performance.md#reopening-a-large-session)

### When the journal compacts

kuku checks whether to seal `events.jsonl` on a clean Session close after all work and writes settle (`Session::close()` in the SDK, or when the server closes an idle Session — exiting a TUI only disconnects the client).

- First seal triggers at **32 MiB** of committed journal data.
- Subsequent seals require the uncompressed tail to reach **both** **32 MiB** and the current on-disk size of the sealed prefix (e.g. a 65 MiB prefix waits for at least 65 MiB of new tail).
- The compressed file replaces the journal atomically only if it is smaller. Full logical history remains readable via [kuku events](../reference/cli.md#kuku-events).

## Model context and compaction

Provider-visible system text, prelude, tools, and admitted canonical messages are frozen into a **context segment**. Messages are admitted once per segment; each prepared Step records segment/message boundaries, resolved route/settings, correlation, and request hash without duplicating full wire requests. Reopening on the same route and capabilities keeps the committed prefix intact.

- **Automatic compaction** starts when the next prompt reaches the configured share of the model’s input budget. The [default threshold](../reference/config.md#handoff) is 95% after reserving output space. The checkpoint summarizes earlier work, records confirmed file paths and line counts, and keeps the current input and recent complete tool batches. Original tool results remain available in the Session.
- **Overflow fallback**: If a provider rejects a request for context size *before* streaming starts and auto-compaction is enabled, the Agent can make one compaction attempt and issue a smaller replacement request in the same Turn (the rejected call still counts against request budgets).
- **Manual compaction & undo**: When idle, run `Session::compact_command` or `/compact` to summarize earlier context without starting a new Turn or reply. Failed or canceled attempts leave existing context untouched. `Session::restore_context_command` or `/compact undo` restores the context from before the latest active checkpoint while keeping later visible messages.
- **Route and capability snapshots.** Each Agent segment records its connection and exact model ID, effective settings, and provider capabilities. API-key rotation can leave the segment intact. A passive change to the endpoint, protocol, model, profile, or capabilities stops projection or dispatch when the recorded route can no longer be used. Select a pair with `configure_agent` or a configured input to open a successor segment that carries portable text and tool history. New Sessions take their default from the running service's config.
- **Legacy segments**: Older segments lacking the newer snapshot fields match on their exact pre-snapshot signature plus recorded connection, protocol, model, and endpoint, keeping strict replay validation (signed native replay and portable text/tool history continue; ambiguous un-replayed thinking or changed legacy routes fail closed rather than fabricating metadata).

Behavioral Prompt assets stay separate from context selection and provider translation. Protocol-native continuation data stays tied to its assistant response and never silently crosses incompatible protocols.

## Message rewind

Rewinding to a finished Turn restores the conversation up to the end of that Turn while keeping the original branch in the same Session (reopening remembers whichever branch is active). The next request reuses the frozen provider prefix and compaction state from that Turn; actual cache TTL depends on the provider.

Optional file restore reverts only verifiable edits made *after* the chosen Turn in the current branch, using versions logged by direct built-in `write_file` and `edit_file` calls (restoring prior contents or removing files created by those calls). Shell and script changes stay on disk, and files whose logged or current content cannot be verified are skipped. Switching back to a previously saved branch restores conversation only. [Terminal controls](../reference/cli.md#rewinding-and-branching) · [History API](../reference/server-api.md#snapshot-history-and-observation)

## Waiting and replies

- Pending permission: answer with `reply_permission` using the exact `request_id` and Agent. [Tool permissions](permissions.md)
- `ask_user`: persists the question once, yields after the current tool batch, and resumes only on a matching `reply_question`.

Waiting Agents do not poll the model or hold a model concurrency permit. Unrelated, conflicting, or stale replies never authorize actions.

## Output and cleanup

Command `stdout`/`stderr` stream from the first byte into bounded frames in a Session-owned file. The model first sees a bounded preview (or recognized build/test summary) with a retained output reference and explicit truncation/filtering flags, and can inspect further via `read_output`. If output exceeds quota, kuku keeps draining the pipes and marks the capture incomplete.

Agents can bind captures to [named Session values](../reference/tools.md#named-session-values). Plain captures may age out, whereas named values and references attached to delivered inputs retain their bytes across reopens (updating a name leaves already-delivered references intact). Write long-lived deliverables as normal workspace files.

Cancellation cleans up process trees via Unix process groups and Windows Job objects (deliberate sandbox escape is outside the trusted local boundary; inherited pipes have a cleanup deadline). Any known incomplete cleanup is persisted and causes cancel/close to return an error.

`Session::close().await` cancels live Jobs, waits for cleanup, and releases the writer lock. Reopening never auto-wakes Agents or replays interrupted side effects. Provider traces are optional diagnostic logs stored separately from Session facts.
