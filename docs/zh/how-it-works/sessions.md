# Sessions

`Session` 是一个目录，也是一本账本。kuku 不会把独立数据库记录当作真相来源。

## Canonical Mental Model

- Session 是一份账本。
- conversation 是账本中的一个聊天线程。
- agent 是从 agent 文件发现的联系人卡片。
- conversation address 是连续性键；复用相同 address 就表示继续同一线程。

## Layout

Session 位于 `$KUKU_HOME` 下按工作区分隔的目录中：

```text
$KUKU_HOME/p/<workspace-path>/sessions/<id>/
|- lock
|- events.jsonl
`- pre-revert-<id>/
```

`events.jsonl` 是持久账本。conversation 通过账本中的事件表达。

## Event Log

`events.jsonl` 的每一行都是一个持久事实。规范的 conversation 事件包括：

- `session.created`
- `conversation.opened`
- `conversation.bound`
- `prompt.snapshot`
- `message.user`
- `message.assistant`
- `turn.started`
- `turn.completed`
- `turn.cancelled`
- `turn.interrupted`
- `context.skills`
- `tool.call`
- `permission.requested`
- `permission.allow`
- `permission.deny`
- `tool.result`
- `handoff`
- `conversation.rollback`
- `conversation.rollback.undone`

完整事件集合见 [Events](../reference/events.md)。

读取方依赖文件顺序。恢复时会忽略末尾不完整的行。

## Conversations Inside One Session

`main` conversation 是 Host 线程。像 `review` 或 `review/api` 这样的 agent conversation 与它一起存在于同一本账本中。

- `main` 是保留的 Host conversation。
- `review` 和 `review/api` 都是有效 conversation address。
- 复用 `review` 会继续该线程。
- 打开 `review/api` 会创建一个新的嵌套线程，它的根联系人是 `review`。

conversation 作用域 replay、notice、中断恢复和 rollback 都是通过把同一本账本过滤到单个 address 来实现的。

## Observability Logs

`$KUKU_HOME/logs/` 是可观测性日志树：

```text
$KUKU_HOME/logs/
|- session/<session-id>.jsonl
|- runtime/<yyyy-mm-dd>.jsonl
`- host/cli|server|webui/<yyyy-mm-dd>.jsonl
```

这些日志用于 host 和 runtime 可见性。保留策略和默认值在 [`[logs]`](../reference/config.md#logs) 中配置。

## Lifecycle

### New session

在没有 session id 的情况下启动运行，会创建新的 Session 目录，并在第一轮之前写入 `session.created`。

### Continuing a session

带已有 session id 启动运行时，会向同一本账本追加新 turn。kuku 会通过回放账本并过滤到当前 conversation 来重建上下文。

如果前一次运行停在 `permission.requested` 之后、`permission.allow`、`permission.deny` 或 `tool.result` 之前，重启时可以从 `events.jsonl` 恢复这个未解决的权限状态。

### Status

Session 状态是账本级别的：

| Status | Meaning |
|---|---|
| `Active` | 存在活动写锁。 |
| `Done` | 不存在锁，且最近一次主线程 turn 的终止事件是完成。 |
| `Interrupted` | 不存在锁，且账本在中途结束或以中断结束。 |

conversation 状态是分开的，可以按 address 列出。见 [Manage Sessions](../guides/manage-sessions.md)。

## Writer Lock

任意时刻只有一个写入者可以向 Session 追加内容。读取操作可以并发进行。

## Replay and Handoff

Replay 是 conversation 作用域的：

- `main` replay 读取历史 host turn 事实和主线程 Tool 活动
- agent conversation replay 只读取该 address 及其 Tool 活动
- handoff 会压缩更早的历史，并为后续 replay 留下摘要边界

后续请求会保留少量最近的 turns，并用 handoff 摘要替换更早的历史。

## Rollback

Rollback 是追加式的。kuku 会记录 rollback 标记事件，而不是删除历史。

conversation rollback 按 address 生效：

| Scope | Effect |
|---|---|
| `messages` | 在未来 replay 中隐藏该 conversation 后续事件。 |
| `file_changes` | 把工作区文件恢复到较早状态，但不隐藏后续消息。 |
| `both` | 同时应用这两种行为。 |

主线程 conversation 的 rollback 可以隐藏较晚的 host turn 事实。agent conversation 的 rollback 只会隐藏该 address 的较晚事件。

文件回滚会使用 `tool.result` 中已经捕获的快照，并把回滚前备份存入 `pre-revert-<id>/`。

## Cancellation and Interruption

- 取消会用 `turn.cancelled` 结束某个 conversation turn。
- 崩溃、在新 turn 前恢复、或运行中途停止，会用 `turn.interrupted` 结束某个 conversation turn。
- runtime notice 可以提示中断 turn、待决权限、打开的 conversation、收件箱消息和已加载 skills。

## Session Operations

Host 可以列出 Session、列出 Session 内的 conversations、检查完整账本、按 conversation 过滤事件、继续某个线程，或删除 Session 目录。

turn 执行流程见 [Agent Loop](agent-loop.md)，Host 暴露方式见 [Host Apps](../architecture/host-apps.md)。
