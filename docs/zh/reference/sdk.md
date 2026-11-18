# SDK

`kuku::Session` 是嵌入入口。[`session_task.rs`](../../../crates/kuku/examples/session_task.rs) 是普通 Rust 调用程序。产品服务使用同一 SDK，并通过 [HTTP 服务接口](server-api.md) 为终端、脚本和浏览器客户端提供服务。

## 操作

| 操作 | 结果 |
|---|---|
| `Session::open(SessionOptions).await` | 获取 writer 并重建事实，不自动激活 |
| `Session::open_for_creation(options, &input, &config).await` | 为首次发送意图认领显式指定的 Session，或在创建中断后重开同一认领 |
| `create_agent(command_id, address, AgentConfig)` | 持久幂等创建回执 |
| `configure_agent(command_id, address, AgentConfig)` | 下一安全边界的 Agent 局部默认值 |
| `send(Input)` / `advance(address)` | 持久投递与明确激活 |
| `send_configured(Input, AgentConfig)` | 一并提交投递和配置，用关联标识配置意图 |
| `agent(address)` / `agents()` | 不消耗输出的派生快照 |
| `agent_view(address)` | 所选 Agent 快照、排队输入和对应的已提交游标 |
| `agent_selection(address)` | 不执行的有效模型设置、能力和最终工具集合 |
| `receipt(command_id)` / `input_receipt(&Input)` | 查询和校验既有回执，不恢复执行 |
| `input_status(address, input_command_id)` | 查询输入已排队、已接纳或已撤回 |
| `withdraw_input(command_id, address, input_command_id)` | 撤回仍在排队的输入并返回其状态 |
| `advance_command(command_id, address)` | 幂等的显式推进 |
| `reply_permission_receipt(...)` / `reply_question_receipt(...)` | 校验既有回复回执，不恢复执行 |
| `events(after, limit)` / `observe(after)` | 有界分页与可续接观察 |
| `read_output(output_id, &OutputRead)` | 有界范围、字面搜索、增量输出 |
| `reply_permission(command_id, address, request_id, allow)` | 精确待决动作回复 |
| `reply_question(command_id, address, request_id, text)` | 以关联输入投递精确问题回复 |
| `cancel(address).await` / `close().await` | 等待所属清理与持久结算 |
| `cancel_execution(command_id, address, execution_id).await` | 取消所观察到的执行并等待清理 |
| `Session::read_agents(events_path)` | 不创建运行资源的离线恢复快照 |
| `Session::read_view(events_path, address)` / `Session::read_receipt(events_path, command_id)` | 从关闭的 Session 读取 Agent 视图或命令回执 |

`SessionOptions::new(workspace, data_dir, config)` 接收具名 provider 配置；可增加 Prompt 资源、限额与宿主 `ToolAuthorizer`。`AgentConfig` 选择 Preset、Tier、具名连接、模型、思考、服务、输出 token、指令、目录和原生 Skill 默认值。连接选择独立于协议。显式 model 覆盖所选 Tier 的模型，保留其余默认值。`AgentConfig.tools` 覆盖 Preset 的最终工具选择；只读 Preset 不会自动增加协作工具。Preset 的指令、Tier 和工具默认值在创建或配置 Agent 时解析并保存；后续修改 Preset 文件不会改变这些已保留默认值。生命周期与回执语义见 [Sessions](../how-it-works/sessions.md)。

`Input.timing` 区分默认的 `InputTiming::NextStep` 和 `InputTiming::NextTurn`。`AgentSnapshot.execution_id` 标识本次执行。`Observation::Live` 包含来源 Agent、执行标识、Turn 和发布该进度时最新的已提交游标。用 `agent_view` 取得与所选 Agent 状态对应的游标，从那里开始观察。

`AgentSnapshot.config` 是请求的配置，`active_config` 记录正在执行或等待中的配置。模型和思考设置可在下一次尚未准备的模型请求生效。工作区、行为和工具设置在下一轮生效。

通过传输接口提供可重试的创建时，`open_for_creation` 会按持久认领核对初始输入、配置和可选 Prompt 目录。宿主仍须使用稳定的命令标识创建 Agent 和提交输入。该 Session 已打开时，使用 `verify_creation(&input, &config, prompts_dir)`。

`Session::read_agents` 从日志重建离线恢复状态，将没有终结事实的 `Running` 映射为 `Interrupted`，即使另一个运行时仍然活跃。查询当前活状态应使用已打开 Session 的 `agent` 或 `agents`。

## 宿主适配器

用 `SessionOptions.model_adapter` 设置 `Arc<dyn ModelAdapter>`，并向 `SessionOptions.tools` 添加 `Arc<dyn ToolAdapter>`。这些工具进入公开 SDK 的同一执行链；[外部调用测试](../../../crates/kuku/tests/adapter_runtime.rs) 覆盖实际文件动作、宿主拒绝、保留输出，以及取消时等待清理。

`ModelAdapter` 接收冻结的 `ModelRequest`，包含选定连接、协议、端点、模型、原生设置、关联和精确请求正文，不包含认证请求头；返回由 `ModelEvent` 组成的 `ModelStream`。配置选择和 `agent_selection` 不读取环境凭据，内置 HTTP 传输只在发送前解析凭据；自定义传输自行负责认证。SDK 继续负责准备屏障、请求额度、模型并发、响应校验、用量与持久日志。丢弃适配器尚未完成的 future 或返回的流时，适配器必须释放其传输资源。

同一个 Agent Turn 内，所有模型 Step 的工具调用 ID 都必须唯一，恢复后也一样。不同 Agent 或 Turn 可以复用 ID。

`ToolAdapter` 在打开时提供不可变的 `ToolSpec`，并接收最终的 `ToolAuthorization` 和 `ToolCancellation` 信号。内置名称不可覆盖。Preset 和 `AgentConfig.tools` 从合并后的可用定义中选择工具。每次调用仍通过宿主授权和持久准备／派发边界。声明的写入与外部动作加入同一 Agent 的工作目录有序动作组；声明的读取不加入该组，可与其他工具重叠执行。工具必须在返回前清理所属工作，取消后也一样；SDK 会等待返回。已知清理不完整通过 `ToolOutput.cleanup_incomplete` 返回，使 `cancel` 或 `close` 失败。

工具正文复用命令输出格式和公开 `read_output` 回退；首次模型视图包含完整引用与截断标志，同时受 `ToolSpec.max_result_bytes` 和运行时预览限额约束。过大的结构化数据保存在输出中，不写进日志。宿主适配器负责返回缓冲区的分配，SDK 的保留输出与模型视图分别有界。

## 工作区执行

模型通过 Session 选定的工具调用 [`run_command` 或 `workspace_script`](tools.md#运行本机程序)。嵌入宿主通过 `Session::events` 或 `Session::observe` 读取 `kuku::event::EventPayload::ToolResult` 的 `structured` 字段，可以用 `kuku::execution::CommandExecutionFacts::from_structured` 解码命令结果。`invocation` 保留程序名及参数边界，或解释器及命令文本。`started`、`termination`、`exit_code` 和 `signal` 描述宿主观察到的进程结果。`capture_complete`、`cleanup_incomplete` 和 `output_ref` 描述保留证据与清理情况。可信宿主可将 `output_ref.id` 交给 `Session::read_output` 分页读取，再用 `OutputSegment::as_bytes()` 获取原始字节。`reported_tests` 包含从程序输出获得的统计。任意外部程序造成的文件变更可能仍然未知。

`workspace_script` 的结果类型为 `kind: "workspace_script_result"`，可以用 `kuku::WorkspaceScriptFacts::from_structured` 解码。结果包含脚本返回的 `value` 及已观察到的宿主 `actions`。每个动作记录 ID、工具、状态、摘要、截断情况及有界结果。动作状态 `unknown` 表示宿主未能确认最终结果。即使脚本失败，也可以结合整次调用的 `timed_out`、`cleanup_incomplete` 和 `not_dispatched` 核对部分完成的工作。见[脚本方法](tools.md#工作区脚本)。

用 `kuku::execution::WorkspaceActionReceipt::from_event` 解码 `workspace.action` 事件，读取日志中脚本内动作的派发和结果事实。在父 `tool_call_id` 内按 `action_id` 关联记录。派发记录缺少对应结果时，副作用仍然未知。

`kuku::execution::FileMutationFacts` 描述写入、精确编辑或移除操作，提供路径、可用的变更前后哈希、字节计数和 `commit_state`。`published_unconfirmed` 表示文件变更已经发生，最终持久化尚未得到确认。重试前应检查文件。

`RuntimeLimits.workspace_script` 限制单次调用的内存、耗时、宿主请求、操作队列和结果大小。在多个 Session 的 `SessionOptions.workspace_host` 中传入同一个 `WorkspaceExecutionHost` 的克隆即可共享容量。进程内默认允许两个脚本 worker 和八个并发工作区操作。脚本宿主动作、直接文件工具、命令和面向模型的输出读取共用操作容量。`WorkspaceExecutionHost::metrics` 可查询当前、峰值和累计启动的 worker 数，无需启动 worker。

## 命名值

调用 `Session::save_named_value(agent, name, source, replace).await` 时，`kuku::session::NamedValueSource` 可选 `Text`、`Json`、`Bytes` 或 `Output`。`Output` 按 ID 绑定已有捕获，无需复制。`Session::named_value` 解析单个名称，`Session::named_values` 列出 Agent 的绑定，无需读取正文。对于已关闭的 Session，`Session::read_named_values(events_path, agent)` 无需打开 writer 即可返回同样的元信息。

用 `Session::read_agent_output(agent, output_id, &OutputRead)` 在 Agent 的权限内读取输出，或用 `Session::read_saved_agent_output` 读取已关闭 Session 的输出。通过 `Session::send_with_values` 或 `Session::send_configured_with_values` 提交已解析的 `OutputReference` 值，将这些引用绑定到输入。见[命名与分享语义](tools.md#命名会话值)。

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

`RuntimeLimits.retry_transient_provider_errors` 默认为 true。如果流尚未打开，且遇到可重试的传输故障或模型服务不可用，SDK 可以重试一次。重试沿用已准备请求，并计入同一请求预算。有界 `session_task` 调用程序将此选项设为 false，关闭该重试。

日志每记录最多 1 MiB。分页最多 256 条，达到 1 MiB 目标停止，单条超出时整页小于 2 MiB。输出读取最多 80,000 字节。保留输出配额含捕获帧及元信息。达到配额时运行时继续排空管道，并将保留结果标记为不完整。Session 默认保留总计 128 MiB 的数据与 64 个命名绑定。没有引用的普通输出保留 24 小时后可被回收。可通过 `RuntimeLimits` 中的 `max_retained_output_bytes`、`max_named_values` 和 `output_retention_secs` 调整这些限额。

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
