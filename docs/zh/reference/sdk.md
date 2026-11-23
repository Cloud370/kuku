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
| `history_points(address, before, limit)` / `jump_history(command_id, address, target_event_id)` | 列出轮次，从已结算的位置继续并保留当前文件 |
| `read_output(output_id, &OutputRead)` | 有界范围、字面搜索、增量输出 |
| `reply_permission(command_id, address, request_id, allow)` | 精确待决动作回复 |
| `reply_question(command_id, address, request_id, text)` | 以关联输入投递精确问题回复 |
| `cancel(address).await` / `close().await` | 等待所属清理与持久结算 |
| `cancel_execution(command_id, address, execution_id).await` | 取消所观察到的执行并等待清理 |
| `Session::read_agents(events_path)` | 不创建运行资源的离线恢复快照 |
| `Session::read_view(events_path, address)` / `Session::read_receipt(events_path, command_id)` | 从关闭的 Session 读取 Agent 视图或命令回执 |

较低层的 `Query` `Run` 通过 non-exhaustive 的 `UiEvent` 和 `ToolEvent` 枚举提供宿主事件；两者匹配时都应保留兜底分支。`UiEvent` 包含 `ModelRequest` 和 `ModelRecovery` 变体；`ModelRequest` 携带请求 ID 和序号，`ModelRecovery` 携带失败／重试请求 ID 与有界重试信息。行为说明见 [Agent 执行循环](../how-it-works/agent-loop.md)。

`SessionOptions::new(workspace, data_dir, config)` 接收具名 provider 配置；可增加 Prompt 资源、限额与宿主 `ToolAuthorizer`。`AgentConfig` 选择 Preset、Tier、具名连接、模型、思考、服务、输出 token、指令、目录和原生 Skill 默认值。连接选择独立于协议。显式 model 覆盖所选 Tier 的模型，保留其余默认值。`AgentConfig.tools` 覆盖 Preset 的最终工具选择；只读 Preset 不会自动增加协作工具。Preset 的指令、Tier 和工具默认值在创建或配置 Agent 时解析并保存；后续修改 Preset 文件不会改变这些已保留默认值。生命周期与回执语义见 [Sessions](../how-it-works/sessions.md)。

`Input.timing` 区分默认的 `InputTiming::NextStep` 和 `InputTiming::NextTurn`。`AgentSnapshot.execution_id` 标识本次执行。`Observation::Live` 包含来源 Agent、执行标识、Turn 和发布该进度时最新的已提交游标。用 `agent_view` 取得与所选 Agent 状态对应的游标，从那里开始观察。

`AgentSnapshot.config` 是请求的配置，`active_config` 记录正在执行或等待中的配置。模型和思考设置可在下一次尚未准备的模型请求生效。工作区、行为和工具设置在下一轮生效。

通过传输接口提供可重试的创建时，`open_for_creation` 会按持久认领核对初始输入、配置和可选 Prompt 目录。宿主仍须使用稳定的命令标识创建 Agent 和提交输入。该 Session 已打开时，使用 `verify_creation(&input, &config, prompts_dir)`。

`Session::read_agents` 从日志重建离线恢复状态，将没有终结事实的 `Running` 映射为 `Interrupted`，即使另一个运行时仍然活跃。查询当前活状态应使用已打开 Session 的 `agent` 或 `agents`。

`SessionHistory::read_with_turns(events_path)` 一次读取历史可见性、Turn 编号和对应的已提交游标。用 `page_before` 分页读取事件，用 `includes` 判断事件是否属于当前历史，用 `rewind_page` 分页查看一次消息回退的内容。`Session::agents_with_cursor` 和 `Session::read_agents_with_cursor` 一次返回 Agent 列表和游标。`session_files` 从文件系统发现日志，`SessionFile::listing` 读取会话选择器需要的标题、状态和等待中的 Agent。

用 `Session::jump_history` 切换到已结束的一轮并保留当前文件。目标在当前历史中时，`Session::preview_file_rewind` 返回工作区相对路径、可用的增删行数、跳过的文件和指纹。用户确认预览后，将指纹传给 `Session::jump_history_with_file_restore`。如果方案已经变化，SDK 会在应用前拒绝。恢复只使用直接调用内置 `write_file` 和 `edit_file` 产生的可核验记录。脚本和 Shell 的改动不在恢复范围内。[Session 行为](../how-it-works/sessions.md#消息回退)

## 宿主适配器

用 `SessionOptions.model_adapter` 设置 `Arc<dyn ModelAdapter>`，并向 `SessionOptions.tools` 添加 `Arc<dyn ToolAdapter>`。这些工具进入公开 SDK 的同一执行链；[外部调用测试](../../../crates/kuku/tests/adapter_runtime.rs) 覆盖实际文件动作、宿主拒绝、保留输出，以及取消时等待清理。

`ModelAdapter` 接收冻结的 `ModelRequest`，包含选定连接、协议、端点、模型、原生设置、关联和精确请求正文，不包含认证请求头；返回由 `ModelEvent` 组成的 `ModelStream`。配置选择和 `agent_selection` 不读取环境凭据，内置 HTTP 传输只在发送前解析凭据；自定义传输自行负责认证。SDK 继续负责准备屏障、请求额度、模型并发、响应校验、用量与持久日志。丢弃适配器尚未完成的 future 或返回的流时，适配器必须释放其传输资源。

同一个 Agent Turn 内，所有模型 Step 的工具调用 ID 都必须唯一，恢复后也一样。不同 Agent 或 Turn 可以复用 ID。

`ToolAdapter` 在打开时提供不可变的 `ToolSpec`，并接收最终的 `ToolAuthorization` 和 `ToolCancellation` 信号。内置名称不可覆盖。Preset 和 `AgentConfig.tools` 从合并后的可用定义中选择工具。每次调用仍通过宿主授权和持久准备／派发边界。声明的写入与外部动作加入同一 Agent 的工作目录有序动作组；声明的读取不加入该组，可与其他工具重叠执行。工具必须在返回前清理所属工作，取消后也一样；SDK 会等待返回。已知清理不完整通过 `ToolOutput.cleanup_incomplete` 返回，使 `cancel` 或 `close` 失败。

工具正文复用命令输出格式和公开 `read_output` 回退；首次模型视图包含完整引用与截断标志，同时受 `ToolSpec.max_result_bytes` 和运行时预览限额约束。过大的结构化数据保存在输出中，不写进日志。宿主适配器负责返回缓冲区的分配，SDK 的保留输出与模型视图分别有界。

## 工作区执行

模型通过 Session 选定的工具调用 [`run_command` 或 `run_js`](tools.md#运行本机程序)。嵌入宿主通过 `Session::events` 或 `Session::observe` 读取 `kuku::event::EventPayload::ToolResult` 的 `structured` 字段，可以用 `kuku::execution::CommandExecutionFacts::from_structured` 解码命令结果。`invocation` 保留程序名及参数边界，或解释器及命令文本。`started`、`termination`、`exit_code` 和 `signal` 描述宿主观察到的进程结果。`capture_complete`、`cleanup_incomplete` 和 `output_ref` 描述保留证据与清理情况。可信宿主可将 `output_ref.id` 交给 `Session::read_output` 分页读取，再用 `OutputSegment::as_bytes()` 获取原始字节。`reported_tests` 包含从程序输出获得的统计。任意外部程序造成的文件变更可能仍然未知。

工具返回 [Session Job](tools.md#session-job) 时，用 `Session::jobs(agent)` 列出最近及正在运行的 Job，或用 `Session::job(agent, job_id)` 查询单个 Job。调用 `Session::wait_job(agent, job_id).await` 等待最终结果，或调用 `Session::cancel_job(agent, job_id).await` 请求取消并等待清理。Job 状态包括 `running`、`completed`、`failed`、`cancelled` 和 `interrupted`。终态的 `JobSnapshot.result` 包含该动作的结构化结果。命令捕获完成后，可将其 `output_id` 传给同一个输出读取器，`output_ref` 提供所捕获的字节范围。

`RuntimeLimits.job_foreground_wait_ms` 控制顶层命令和脚本的前台观察窗口。`RuntimeLimits.max_command_seconds` 可为每条命令设置独立的进程截止时间，包括 `run_js` 内启动的命令和作为 Job 运行的命令。默认值为 `None`。

`run_js` 的结果类型为 `kind: "workspace_script_result"`，可以用 `kuku::WorkspaceScriptFacts::from_structured` 解码。结果包含脚本内联返回的 `value` 及已观察到的宿主 `actions`。完整 JSON 单独保留时，`value` 为空，`output_ref` 提供读取位置。每个动作记录 ID、工具、状态、摘要、截断情况及有界结果。动作状态 `unknown` 表示宿主未能确认最终结果。即使脚本失败，也可以结合整次调用的 `timed_out`、`cleanup_incomplete` 和 `not_dispatched` 核对部分完成的工作。见[脚本方法](tools.md#javascript-组合工具)。

用 `kuku::execution::WorkspaceActionReceipt::from_event` 解码 `workspace.action` 事件，读取日志中脚本内动作的派发和结果事实。在父 `tool_call_id` 内按 `action_id` 关联记录。派发记录缺少对应结果时，副作用仍然未知。

`kuku::execution::FileMutationFacts` 描述写入、精确编辑或移除操作，提供路径、可用的变更前后哈希、字节计数和 `commit_state`。`published_unconfirmed` 表示文件变更已经发生，最终持久化尚未得到确认。重试前应检查文件。

`RuntimeLimits.workspace_script` 限制单次调用的内存、JavaScript 执行时间、宿主请求、操作队列和结果大小。等待宿主动作的时间不计入 JavaScript 执行预算。在多个 Session 的 `SessionOptions.workspace_host` 中传入同一个 `WorkspaceExecutionHost` 的克隆即可共享容量。进程内默认允许两个脚本 worker 和八个并发工作区操作。脚本宿主动作、直接文件工具、命令和面向模型的输出读取共用操作容量。`WorkspaceExecutionHost::metrics` 可查询当前、峰值和累计启动的 worker 数，无需启动 worker。

## 命名值

调用 `Session::save_named_value(agent, name, source, replace).await` 时，`kuku::session::NamedValueSource` 可选 `Text`、`Json`、`Bytes` 或 `Output`。`Output` 按 ID 绑定已有捕获，无需复制。`Session::named_value` 解析单个名称，`Session::named_values` 列出 Agent 的绑定，无需读取正文。对于已关闭的 Session，`Session::read_named_values(events_path, agent)` 无需打开 writer 即可返回同样的元信息。

用 `Session::read_agent_output(agent, output_id, &OutputRead)` 在 Agent 的权限内读取输出，或用 `Session::read_saved_agent_output` 读取已关闭 Session 的输出。通过 `Session::send_with_values` 或 `Session::send_configured_with_values` 提交已解析的 `OutputReference` 值，将这些引用绑定到输入。见[命名与分享语义](tools.md#命名会话值)。

## 调用程序

```bash
cargo run -p kuku --example session_task -- \
  --workspace /path/to/work --prompt-file /path/to/task.txt \
  --config /path/to/config.toml --data-dir /path/to/session-data \
  --connection deepseek --reasoning-effort low \
  --output /path/to/result.json
```

使用 `--session-id` 重开，使用 `--command-id` 在回执丢失后重试。结果包含持久模型、工具和 Turn 事实数、日志路径和游标、provider 设置和报告、限额、耗时和清理状态。`runtime_completed` 表示执行结算，benchmark 仍需实际判定器。`--max-requests N` 可限制本次打开期间的尝试次数。省略时，示例用 `null` 报告 `max_requests`。`--max-agents` 设置保留的 Agent 上限。`--max-context-tokens` 按 token 设置配置上下文窗口的上限。`--max-request-bytes` 独立限制序列化请求字节数，省略时继承 SDK 默认值。

## 限额与用量

默认限额允许 64 个 Agent、每 Agent 128 条待处理输入、65,536 输入字节、四个模型流、每 Agent 八个工具、每份保留输出 8 MiB、首次预览 8 KiB 和 128 条临时观察。宿主需要限制传输大小时，可通过 `RuntimeLimits::max_request_bytes` 设置序列化请求上限。预览至少 1 KiB，以保留完整引用和标志。

`RuntimeLimits.max_requests_per_open` 可限制所有 Agent Turn 的总尝试数，重开后重置。`RuntimeLimits.max_output_tokens_per_request` 可为宿主上的每次响应设置生成上限。两者默认都是 `None`。Agent 显式设置的输出上限优先于 tier 配置，宿主上限适用于每个 Agent。Rust 的 `TierConfig.max_output_tokens`、`AgentSelection.max_output_tokens` 和 `ModelSelection.max_output_tokens` 类型均为 `Option<u32>`。协议允许时，省略有效值就由供应商选择默认行为。请求准备记录给出本次请求采用的值。`session_task --max-output-tokens N` 同时设置宿主和主 Agent 的上限。详见[模型配置](config.md#modelname)。

`RuntimeLimits.retry_transient_provider_errors` 默认为 true。最多进行六次尝试（五次重试），所有可重试的 provider 故障共用 2/4/8/16/32 秒指数退避，并加 0–25% 抖动。有效且不超过 30 秒的 `Retry-After` 可延长等待；缺失、无效或更长的提示不会禁用重试，超过 30 秒的提示会被忽略。传输层将明确的认证、权限、额度、上下文、请求和无效响应错误视为终止条件。首个流内容前的错误可以重试；输出开始后发生的可重试错误会结束本次尝试，以免重复已发出或持久化的副作用。每次重试沿用已准备请求，只有实际派发的尝试计入调用方请求预算。可将此选项设为 false 或使用 `session_task --no-provider-retry` 关闭重试。示例在 `limits.automatic_retries` 中报告五次允许的重试（禁用时为零）。

开启 provider 重试时，如果一次完整的工具调用响应包含无效 JSON 参数，Session 可以请求模型重新生成响应。整批调用会在派发之前丢弃，重生成使用已经接纳的上下文。每步最多重生成五次，重开 Session 后也沿用这个计数。每接纳一批工具调用就进入新一步。每次重生成计入调用方的模型请求预算。日志记录失败类别、工具调用索引、可识别的已注册工具名和当前步骤的尝试次数，省去被拒绝的参数原文。

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
  --max-requests 16 --timer-ms 250 \
  --max-output-tokens 32768 --max-context-tokens 131072 \
  --data-dir /path/to/session-data --output /path/to/result.json
```

宿主用 `Activation::Enqueue` 投递前两个输入，分别显式推进接收者，再通过带关联的 `InputSource::External("timer")` 投递定时器输入。每次投递都有稳定的命令 ID，并核对相同重试的回执。示例固定持有两个 Agent，禁用模型创建 Agent 和原生 Skill，分别限制两次打开期间的模型请求总数。重开会续用宿主新授权的请求额度。输出和上下文默认值为 32,768 和 131,072 tokens。使用 `--max-request-bytes` 可设置宿主请求字节上限。

宿主负责定时器的生命周期，并在重开后启动它；Session 持久化已投递的输入。观察者在执行稳定后才消费事件，因此观察订阅不会驱动工作。JSON 结果保留逐轮回执与快照、provider 和工具事实、有界的保留命令输出、休眠请求计数，以及关闭和重开证据。`runtime_completed` 要求三轮都稳定为空闲、没有待处理输入，并成功完成持久化关闭；任务产物仍需独立验证。等待中的 Agent 会报告为未完成。
