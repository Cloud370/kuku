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
| `agent_selection(address)` | 不执行的有效模型设置、能力和最终工具集合 |
| `receipt(command_id)` / `input_receipt(&Input)` | 查询和校验既有回执，不恢复执行 |
| `reply_permission_receipt(...)` / `reply_question_receipt(...)` | 校验既有回复回执，不恢复执行 |
| `events(after, limit)` / `observe(after)` | 有界分页与可续接观察 |
| `read_output(output_id, &OutputRead)` | 有界范围、字面搜索、增量输出 |
| `reply_permission(command_id, address, request_id, allow)` | 精确待决动作回复 |
| `reply_question(command_id, address, request_id, text)` | 以关联输入投递精确问题回复 |
| `cancel(address).await` / `close().await` | 等待所属清理与持久结算 |
| `Session::read_agents(events_path)` | 不创建运行资源的离线恢复快照 |

较低层的 `Query` `Run` 通过 non-exhaustive 的 `UiEvent` 和 `ToolEvent` 枚举提供宿主事件；两者匹配时都应保留兜底分支。两个枚举都包含 `ModelRequest` 和 `ModelRecovery` 变体；`ModelRequest` 携带请求 ID 和序号，`ModelRecovery` 携带失败／重试请求 ID 与有界重试信息。行为说明见 [Agent 执行循环](../how-it-works/agent-loop.md)。

`SessionOptions::new(workspace, data_dir, config)` 接收具名 provider 配置；可增加 Prompt 资源、限额与宿主 `ToolAuthorizer`。`AgentConfig` 选择 Preset、Tier、具名连接、模型、思考、服务、输出 token、指令、目录和原生 Skill 默认值。连接选择独立于协议。显式 model 覆盖所选 Tier 的模型，保留其余默认值。`AgentConfig.tools` 覆盖 Preset 的最终工具选择；只读 Preset 不会自动增加协作工具。Preset 的指令、Tier 和工具默认值在创建或配置 Agent 时解析并保存；后续修改 Preset 文件不会改变这些已保留默认值。生命周期与回执语义见 [Sessions](../how-it-works/sessions.md)。

`Session::read_agents` 从日志重建离线恢复状态，将没有终结事实的 `Running` 映射为 `Interrupted`，即使另一个运行时仍然活跃。查询当前活状态应使用已打开 Session 的 `agent` 或 `agents`。

## 宿主适配器

用 `SessionOptions.model_adapter` 设置 `Arc<dyn ModelAdapter>`，并向 `SessionOptions.tools` 添加 `Arc<dyn ToolAdapter>`。这些工具进入公开 SDK 的同一执行链；[外部调用测试](../../../crates/kuku/tests/adapter_runtime.rs) 覆盖实际文件动作、宿主拒绝、保留输出，以及取消时等待清理。

`ModelAdapter` 接收冻结的 `ModelRequest`，包含选定连接、协议、端点、模型、原生设置、关联和精确请求正文，不包含认证请求头；返回由 `ModelEvent` 组成的 `ModelStream`。配置选择和 `agent_selection` 不读取环境凭据，内置 HTTP 传输只在发送前解析凭据；自定义传输自行负责认证。SDK 继续负责准备屏障、请求额度、模型并发、响应校验、用量与持久日志。丢弃适配器尚未完成的 future 或返回的流时，适配器必须释放其传输资源。

`ToolAdapter` 在打开时提供不可变的 `ToolSpec`，并接收最终的 `ToolAuthorization` 和 `ToolCancellation` 信号。内置名称不可覆盖。Preset 和 `AgentConfig.tools` 从合并后的可用定义中选择工具。每次调用仍通过宿主授权和持久准备／派发边界。声明的写入与外部动作加入同一 Agent 的工作目录有序动作组；声明的读取不加入该组，可与其他工具重叠执行。工具必须在返回前清理所属工作，取消后也一样；SDK 会等待返回。已知清理不完整通过 `ToolOutput.cleanup_incomplete` 返回，使 `cancel` 或 `close` 失败。

工具正文复用命令输出格式和公开 `read_output` 回退；首次模型视图包含完整引用与截断标志，同时受 `ToolSpec.max_result_bytes` 和运行时预览限额约束。过大的结构化数据保存在输出中，不写进日志。宿主适配器负责返回缓冲区的分配，SDK 的保留输出与模型视图分别有界。

## 调用程序

```bash
cargo run -p kuku --example session_task -- \
  --workspace /path/to/work --prompt-file /path/to/task.txt \
  --config /path/to/config.toml --data-dir /path/to/session-data \
  --connection deepseek --reasoning-effort low \
  --max-steps 12 --max-requests 32 --max-output-tokens 32768 --max-context-tokens 131072 \
  --output /path/to/result.json
```

使用 `--session-id` 重开，使用 `--command-id` 在回执丢失后重试。结果包含持久模型／工具／Turn 事实数、日志路径／游标、provider 设置／报告、限额、耗时和清理状态。`runtime_completed` 表示执行结算，benchmark 仍需实际判定器。`--max-requests` 限制本次打开期间所有 Agent 的总请求，默认等于 `--max-steps`。`--max-agents` 设置保留的 Agent 上限。不自动重试付费请求。请求序列化字节数受 `--max-context-tokens` 约束，作为保守输入 token 上界。

## 限额与用量

默认：64 个 Agent、每 Agent 128 条待处理输入、65,536 输入字节、每请求 512 KiB、四个模型流、每 Turn 20 次请求、每 Agent 八个工具、每份保留输出 8 MiB、首次预览 16 KiB、128 条临时观察。`RuntimeLimits.max_output_tokens_per_request` 可限制每个 Agent 请求的有效输出 token；显式 Agent 值取 `min`，选择查询和 prepared 事实反映有效值。`session_task --max-output-tokens` 同时设置宿主上限与主 Agent 选择。`RuntimeLimits.max_requests_per_open` 可限制所有 Agent Turn 的总尝试数，重开后重置，超额工作在发送前拒绝；默认 `None`。零值或不支持边界会失败；预览至少 1 KiB，保留完整引用与标志。

日志每记录最多 1 MiB。分页最多 256 条，达到 1 MiB 目标停止，单条超出时整页小于 2 MiB。输出读取最多 80,000 字节。保留输出配额含帧／元信息，超过后标记不完整并继续排空。这些是每份输出的边界，宿主选择 Session 保留与总磁盘容量。

`UsageTotals` 将缺失计数保留为 `None`；`unreported_requests` 标识输入／输出报告不完整。输入总量包含缓存，缓存计数是其子集。原始可选 provider 报告分别持久保存。已知清理不完整使 `cancel` 或 `close` 失败。

## 受控讨论与定时器

[`session_orchestrate.rs`](../../../crates/kuku/examples/session_orchestrate.rs) 是一个小型宿主示例，在同一 Session 中使用两个独立 Agent。它恰好准入三轮工作：writer 起草、reviewer 根据选定的 writer 结果评审，以及关闭并重开 Session、等待宿主定时器后的 writer 续作。reviewer 只接收自己的指令和选定结果；writer 在重开后保留自己的历史。

用三个文本文件分别提供任务、评审指令和续作指令。例如，让 writer 检查并修复一个小程序、运行测试，让 reviewer 检查共享文件，再让 writer 按选定评审修改并验证结果。使用[配置](config.md)中的具名连接，凭据通过环境变量引用提供。

```bash
cargo run -p kuku --example session_orchestrate -- \
  --workspace /path/to/work --config /path/to/config.toml \
  --prompt-file /path/to/draft.txt \
  --review-prompt-file /path/to/review.txt \
  --continue-prompt-file /path/to/continue.txt \
  --writer-connection direct --writer-tier balanced \
  --reviewer-connection openrouter-openai-responses --reviewer-tier light \
  --max-steps 8 --max-requests 16 --timer-ms 250 \
  --max-output-tokens 32768 --max-context-tokens 131072 \
  --data-dir /path/to/session-data --output /path/to/result.json
```

宿主用 `Activation::Enqueue` 投递前两个输入，分别显式推进接收者，再通过带关联的 `InputSource::External("timer")` 投递定时器输入。每次投递都有稳定的命令 ID，并核对相同重试的回执。示例固定持有两个 Agent，禁用模型创建 Agent 和原生 Skill，分别限制两次打开期间的模型请求总数。重开会续用宿主新授权的请求额度。输出和上下文默认值保持为 32,768 和 131,072 tokens。

宿主负责定时器的生命周期，并在重开后启动它；Session 持久化已投递的输入。观察者在执行稳定后才消费事件，因此观察订阅不会驱动工作。JSON 结果保留逐轮回执与快照、provider 和工具事实、有界的保留命令输出、休眠请求计数，以及关闭和重开证据。`runtime_completed` 要求三轮都稳定为空闲、没有待处理输入，并成功完成持久化关闭；任务产物仍需独立验证。等待中的 Agent 会报告为未完成。
