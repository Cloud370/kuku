# SDK

`kuku::Session` 是嵌入入口。[`session_task.rs`](../../../crates/kuku/examples/session_task.rs) 是普通 Rust 调用程序，与 CLI 使用相同操作。

## 操作

| 操作 | 结果 |
|---|---|
| `Session::open(SessionOptions).await` | 获取 writer 并重建事实，不自动激活 |
| `create_agent(command_id, address, AgentConfig)` | 持久幂等创建回执 |
| `configure_agent(command_id, address, AgentConfig)` | 下一安全边界的 Agent 局部默认值 |
| `send(Input)` / `advance(address)` | 持久投递与明确激活 |
| `agent(address)` / `agents()` | 不消耗输出的派生快照 |
| `events(after, limit)` / `observe(after)` | 有界分页与可续接观察 |
| `read_output(output_id, &OutputRead)` | 有界范围、字面搜索、增量输出 |
| `reply_permission(command_id, address, request_id, allow)` | 精确待决动作回复 |
| `reply_question(command_id, address, request_id, text)` | 以关联输入投递精确问题回复 |
| `cancel(address).await` / `close().await` | 等待所属清理与持久结算 |
| `Session::read_agents(events_path)` | 不创建运行资源的只读快照 |

`SessionOptions::new(workspace, data_dir, config)` 接收具名 provider 配置；可增加 Prompt 资源、限额与宿主 `ToolAuthorizer`。`AgentConfig` 选择 Preset、Tier、具名连接、模型、思考、服务、输出 token、指令、目录和原生 Skill 默认值。连接选择独立于协议。生命周期与回执语义见 [Sessions](../how-it-works/sessions.md)。

## 调用程序

```bash
cargo run -p kuku --example session_task -- \
  --workspace /path/to/work --prompt-file /path/to/task.txt \
  --config /path/to/config.toml --data-dir /path/to/session-data \
  --connection deepseek --reasoning-effort low \
  --max-steps 12 --max-output-tokens 32768 --max-context-tokens 131072 \
  --output /path/to/result.json
```

使用 `--session-id` 重开，使用 `--command-id` 在回执丢失后重试。结果包含持久模型／工具／Turn 事实数、日志路径／游标、provider 设置／报告、限额、耗时和清理状态。`runtime_completed` 表示执行结算，benchmark 仍需实际判定器。不自动重试付费请求。请求序列化字节数受 `--max-context-tokens` 约束，作为保守输入 token 上界。

## 限额与用量

默认：64 个 Agent、每 Agent 128 条待处理输入、65,536 输入字节、每请求 512 KiB、四个模型流、每 Turn 20 次请求、每 Agent 八个工具、每命令输出 8 MiB、首次预览 16 KiB、128 条临时观察。零值或不支持边界会失败；预览至少 1 KiB，保留完整引用与标志。

日志每记录最多 1 MiB。分页最多 256 条，达到 1 MiB 目标停止，单条超出时整页小于 2 MiB。输出读取最多 80,000 字节。命令配额含帧／元信息，超过后标记不完整并继续排空。这些是每命令边界，宿主选择 Session 保留与总磁盘容量。

`UsageTotals` 将缺失计数保留为 `None`；`unreported_requests` 标识输入／输出报告不完整。输入总量包含缓存，缓存计数是其子集。原始可选 provider 报告分别持久保存。已知清理不完整使 `cancel` 或 `close` 失败。
