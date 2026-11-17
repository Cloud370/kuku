# SDK

`kuku::Session` is the embedding entry point. [`session_task.rs`](../../../crates/kuku/examples/session_task.rs) is an ordinary Rust caller using the same operations as the CLI.

## Operations

| Operation | Result |
|---|---|
| `Session::open(SessionOptions).await` | Acquire writer ownership and reconstruct facts without automatic activation |
| `create_agent(command_id, address, AgentConfig)` | Durable idempotent creation receipt |
| `configure_agent(command_id, address, AgentConfig)` | Agent-local defaults for the next safe boundary |
| `send(Input)` / `advance(address)` | Durable delivery and explicit activation |
| `agent(address)` / `agents()` | Query derived snapshots without consuming output |
| `events(after, limit)` / `observe(after)` | Bounded pages and reconnectable observation |
| `read_output(output_id, &OutputRead)` | Bounded range, literal search, or incremental output |
| `reply_permission(command_id, address, request_id, allow)` | Exact pending-action reply |
| `reply_question(command_id, address, request_id, text)` | Exact question reply delivered as correlated input |
| `cancel(address).await` / `close().await` | Await owned cleanup and durable settlement |
| `Session::read_agents(events_path)` | Read snapshots without creating runtime resources |

`SessionOptions::new(workspace, data_dir, config)` accepts named-provider configuration. Options add prompt assets, resource limits, and a host `ToolAuthorizer`. `AgentConfig` selects preset, tier, named connection, model, reasoning, service tier, output tokens, instructions, directory, and native skill defaults. Connection selection is independent of wire protocol. An explicit model overrides the selected tier’s model while retaining its other defaults. `AgentConfig.tools` overrides the preset’s final tool selection; collaboration tools are not added to a read-only preset. Lifecycle and receipt semantics live in [Sessions](../how-it-works/sessions.md).

## Calling program

```bash
cargo run -p kuku --example session_task -- \
  --workspace /path/to/work --prompt-file /path/to/task.txt \
  --config /path/to/config.toml --data-dir /path/to/session-data \
  --connection deepseek --reasoning-effort low \
  --max-steps 12 --max-requests 32 --max-output-tokens 32768 --max-context-tokens 131072 \
  --output /path/to/result.json
```

Use `--session-id` to reopen and `--command-id` to retry delivery after losing a receipt. Results include durable model/tool/turn fact counts, journal path/cursor, provider settings/reports, limits, duration, and cleanup. `runtime_completed` means execution settled; a benchmark still needs its actual judge. `--max-requests` limits all Agent requests across this open runtime; it defaults to `--max-steps`. `--max-agents` sets the retained Agent limit. Automatic paid retries are disabled. Serialized request bytes are capped by `--max-context-tokens`, a conservative input-token bound.

## Limits and accounting

Defaults: 64 agents, 128 pending inputs per agent, 65,536 input bytes, 512 KiB per prepared request, four simultaneous model streams, 20 requests per turn, eight tools per agent, 8 MiB per command output, 16 KiB first preview, and 128 transient observations. `RuntimeLimits.max_requests_per_open` optionally caps total attempts across all Agent turns, resets on reopen, and rejects excess work before dispatch; its default is `None`. Zero or unsupported bounds fail validation; output previews require at least 1 KiB for a complete reference and flags.

Journal records are capped at 1 MiB. Pages contain at most 256 records and stop at a 1 MiB target, with one-record overrun below 2 MiB. Output reads return at most 80,000 bytes. Command quota includes framing/metadata and marks incomplete capture while continuing to drain. These are per-command bounds; hosts choose session retention and total disk capacity.

`UsageTotals` retains absent counters as `None`; `unreported_requests` identifies incomplete input/output reporting. Input totals include cached input, so cache counters are subsets. Raw optional provider reports remain separate facts. Known incomplete cleanup makes `cancel` or `close` fail.
