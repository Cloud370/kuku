# Events

`events.jsonl` 是单个 Session 的追加式账本。Session 是一份账本，conversation 是这份账本中的一个聊天线程。

## Naming Rule

```text
<domain>.<action>
```

全部小写，使用点分隔。

## Canonical Mental Model

- `session`：一段运行历史的持久账本
- `conversation`：账本中的一个聊天线程
- `agent`：可拥有一个或多个 conversation 的联系人卡片
- `address`：conversation 的连续性标识；重复使用同一个 address 就表示延续同一线程

## Canonical Event Kinds

| Event | Meaning |
|---|---|
| `session.created` | 账本元数据。新 Session 的首个规范事件。 |
| `conversation.opened` | 某个 conversation address 首次在账本中打开。 |
| `conversation.bound` | 某个 conversation 绑定到一个 agent 身份快照。 |
| `prompt.snapshot` | 某个 conversation turn 的 prompt 重建输入。 |
| `message.user` | 某个 conversation 中的一条用户消息。 |
| `message.assistant` | 某个 conversation 中的一条助手消息。 |
| `turn.started` | 某个 conversation turn 开始。 |
| `turn.completed` | 某个 conversation turn 正常结束。 |
| `turn.cancelled` | 某个 conversation turn 被取消结束。 |
| `turn.interrupted` | 某个 conversation turn 被中断结束。 |
| `context.sources` | 一次主线程 turn 重建时使用的指令与 memory 来源文件。 |
| `context.skills` | 某个 conversation turn 的 skill registry 快照和 bootstrap skill 列表。 |
| `tool.call` | 一次请求的 Tool 调用。 |
| `permission.requested` | 某次 Tool 调用的持久待决权限状态。 |
| `permission.allow` | Tool 授权允许决策。 |
| `permission.deny` | Tool 授权拒绝决策。 |
| `tool.result` | 一次 Tool 调用的结果。 |
| `handoff` | 用于未来 replay 的摘要边界。 |
| `conversation.rollback` | conversation 作用域回滚标记。 |
| `conversation.rollback.undone` | 撤销一次 conversation rollback。 |

## Common Fields

每条持久化事件至少都包含：

| Field | Meaning |
|---|---|
| `id` | Session 账本内单调递增的整数 |
| `kind` | 事件类型 |
| `ts` | ISO 8601 时间戳 |

conversation 作用域事件还会带上 `conversation`。turn 作用域事件还会带上 `turn`。

## Required Fields By Event

| Event | Required fields |
|---|---|
| `session.created` | `ts`, `schema_version`, `session_id`, `created_at`, `kuku_version` |
| `conversation.opened` | `ts`, `conversation` |
| `conversation.bound` | `ts`, `conversation`, `binding_id` |
| `prompt.snapshot` | `ts`, `conversation`, `binding_id`, `snapshot_id`, `turn`, `messages`, `project_instruction_sources`, `memory_sources`, `prompt_asset_sources`, `skills`, `bootstrap_loaded`, `provider`, `model`, `renderer`, `tool_registry`, `capabilities` |
| `message.user` | `ts`, `conversation`, `turn`, `text` |
| `message.assistant` | `ts`, `conversation`, `turn`, `message_id`, `text` |
| `turn.started` | `ts`, `conversation`, `turn` |
| `turn.completed` | `ts`, `conversation`, `turn` |
| `turn.cancelled` | `ts`, `conversation`, `turn`, `reason` |
| `turn.interrupted` | `ts`, `conversation`, `turn`, `reason` |
| `context.sources` | `turn`, `ts`, `request_id`, `project_instruction_sources`, `memory_sources` |
| `context.skills` | `conversation`, `turn`, `ts`, `registry`, `bootstrap_loaded` |
| `tool.call` | `turn`, `ts`, `tool_call_id`, `request_id`, `index`, `tool`, `args` |
| `permission.requested` | `turn`, `ts`, `tool_call_id`, `tool`, `risk`, `summary`, `candidate`, `source` |
| `permission.allow` | `turn`, `ts`, `tool_call_id`, `tool`, `scope`, `matcher`, `source` |
| `permission.deny` | `turn`, `ts`, `tool_call_id`, `tool`, `reason`, `source` |
| `tool.result` | `turn`, `ts`, `tool_call_id`, `status`, `summary`, `model_content`, `truncated`, `files_read`, `files_changed`, `commands_run` |
| `handoff` | `turn`, `ts`, `request_id`, `summary`, `keep_turns` |
| `conversation.rollback` | `ts`, `conversation`, `to_turn`, `to_event_id`, `scope` |
| `conversation.rollback.undone` | `ts`, `conversation`, `rollback_event_id` |

`tool.call` 和 `tool.result` 可以选择性包含 `conversation`。省略时表示属于 `main` conversation。

## Rollback Scope Values

Rollback 事件会记录以下 scope 之一：

- `messages`
- `file_changes`
- `both`

`messages` 会让该 conversation 后续事件不再参与未来 replay。`file_changes` 只回退工作区文件，不隐藏 conversation 历史。`both` 同时执行这两件事。

## Permission State

`permission.requested` 是持久的待决状态，不是可观测性记录。Host 决定结果后，kuku 会在 `tool.result` 之前追加 `permission.allow` 或 `permission.deny`。

## Session Facts vs Runtime Streams

并非所有运行时事件都是 Session 事实。流式文本、实时命令输出、Tool 进度和 host 可见日志记录都属于 runtime stream 事件，不写入 `events.jsonl`。

HTTP wire 事件见 [Server API](server-api.md)。
