# CLI

## 入口模式

- `kuku` 在交互终端打开全屏客户端。
- `kuku run ...` 执行非交互任务，适用于脚本或没有交互终端的环境。

终端、脚本和浏览器客户端都向服务提交任务。本地客户端复用所选数据目录的受管服务。[运行服务](../guides/run-the-server.md)

## 连接与工作区选项

| 选项 | 含义 |
|---|---|
| `-w`、`--workspace <path>` | 命令和文件工具使用的工作区。本地默认使用解析后的当前目录，远程 Session 需要指定服务端机器上的绝对路径。 |
| `--data-dir <path>` | 本地服务的数据目录。默认为 `$KUKU_HOME` 或 `~/.kuku`。使用 `--remote` 时，该选项选择客户端的本地存储目录。 |
| `--config <path>` | 选择本地配置文件。远程配置在服务端机器上管理。 |
| `--remote <url>` | 显式连接该服务地址。 |
| `--token <token>` | 显式连接的服务访问令牌，也可设置 `KUKU_TOKEN`。 |

运行 `kuku run` 时把选项放在提示词之前。命令会将剩余的词作为提示词。

## 顶层命令

| 命令 | 用途 |
|---|---|
| `kuku run <prompt...>` | 提交任务并观察执行 |
| `kuku reply <session-id> <request-id>` | 回复持久问题或授权请求 |
| `kuku show <session-id>` | 查看 Session 的最终输出 |
| `kuku events <session-id>` | 查看持久 Session 事实 |
| `kuku list` | 列出 Session |
| `kuku pending ...` | 核对丢失回执的提交 |
| `kuku delete <session-id>` | 删除 Session |
| `kuku config ...` | 查看、验证或修改配置 |
| `kuku init` | 初始化配置和运行目录 |
| `kuku prompts ...` | 查看或导出 Prompt 资产 |
| `kuku agents ...` | 列出或查看 Agent 联系卡 |
| `kuku skills ...` | 列出或查看 Skill |
| `kuku server ...` | 显式启动 HTTP 服务 |
| `kuku server status` | 检查服务身份和当前活动 |
| `kuku server restart` | 在手头的工作结束后重启受管本地服务 |
| `kuku web ...` | 打开所选服务的浏览器界面 |

## 终端界面

| 按键或操作 | 行为 |
|---|---|
| Enter | 发送草稿。Agent 忙时在下一个安全的模型请求边界接纳，如果当前轮先结束则在下一轮接纳。 |
| Alt+Enter | 把草稿排到下一轮。 |
| Shift+Enter 或 Ctrl+J | 在草稿中换行。 |
| Esc | 关闭当前获得焦点的菜单，或打断所选执行并保留草稿。 |
| 草稿为空时按 `/`，或按 F1 | 搜索可用操作和 Skill。选中 Skill 会在草稿中加入可编辑的 `/skill:NAME` 前缀，用于下一次发送。 |
| `@` 或 Tab | 将工作区相对路径补全到草稿。 |
| `/new`、`/resume` | 打开空白草稿或查找已有 Session。 |
| `/agents` | 选择接收者或在 Session 中新建 Agent。 |
| `/queue`、`/withdraw`、`/continue` | 排到下一轮、撤回排队输入，或显式继续工作。 |
| F2 或 `/reply` | 回复所选 Agent 的待决问题或授权。 |
| `/pending` | 检查、重试或丢弃未确认的提交。 |
| `/model`、`/thinking` | 为所选 Agent 选择模型或思考设置。 |
| Ctrl+O、`/tools`、`/copy` | 展开或收起内联工具详情、读取保留输出，或复制回复正文。 |
| 鼠标左键拖动 | 高亮选区，松开时将所选文字复制到剪贴板。 |
| Page Up、Page Down、Ctrl+End | 滚动对话或回到最新输出。 |
| Ctrl+P、Ctrl+N | 在编辑区调出上一条或下一条已发送的提示词。 |
| `/server`、`/reconnect`、`/restart` | 检查所连服务、重新连接，或重启受管本地服务。 |
| `/web`、`/quit` | 在浏览器中打开对话或退出终端客户端。Ctrl+C 也可以退出终端客户端。 |

## `kuku run`

```text
kuku run [options] <prompt...>
```

参数：

| 参数 | 含义 |
|---|---|
| `-y`, `--yes` | 显式选择默认的 trusted-local 策略 |
| `--connection <name>` | 精确的已配置连接名称 |
| `--model <id>` | 精确的模型标识 |
| `--tier <name>` | 可复用的模型路由和设置 |
| `--agent <address>` | Session 内稳定地址，默认 `main` |
| `--command-id <id>` | 调用方指定的输入幂等标识 |
| `--reasoning-effort <value>` | Provider 支持的思考设置 |
| `--service-tier <value>` | Provider 支持的服务设置 |
| `--max-output-tokens <n>` | 单次模型响应的 token 上限 |
| `-s`, `--session <id>` | 继续指定 Session |
| `-c`, `--continue` | 继续最近的 Session |
| `--json` | 输出一行最终 JSON |
| `--stream-json` | 实时输出 JSON 行 |
| `--show-thinking` | 显示思考内容 |
| `--raw` | 纯文本输出 |
| `--verbose` | 显示持久 Agent 快照、配置和用量 |
| `--prompts-dir <dir>` | 创建 Session 时覆盖内嵌 Prompt 资产。使用 `--remote` 时，填写服务端已授权的绝对目录。 |
| `--no-agents` | 禁用 `create_agent`、`send_message` 和 `list_agents`，保留 `ask_user` |
| `--no-skills` | 禁用默认 Skill 工具 `list_skills`、`search_skills`、`use_skill` 及斜杠 Skill 加载 |

服务默认使用 SDK 的 trusted-local 策略。执行开始后，按 Ctrl-C 会请求取消观察到的执行并等待清理。客户端连接丢失时，服务会继续执行。新的 `run` 可以推进已停止的 Agent。等待中的 Agent 需要匹配的回复。请检查实际文件或命令结果，确认任务是否成功。

Tier 名称使用 `--tier`。`--model` 选择精确模型标识，`--connection` 即使在多个连接共用协议时也只选择指定名称。显式 `--model` 覆盖 Tier 的模型，重开 Agent 时同时提供这两个参数也遵循此优先级。显式设置只影响所选 Agent。重开时不传选择参数，会保留该 Agent 的配置与独立上下文。切换模型路由会清除该 Agent 先前的 reasoning/service 覆盖值，再应用新的显式设置。

使用 `--command-id` 重试同一输入时，返回原始持久回执和当前 Agent 状态。如果首次发送丢失了响应，客户端会为这条未确认命令保留生成的 Session ID。取得回执后，后续重试使用返回的 ID 作为 `--session`。重放时，`--json` 和 `--stream-json` 返回 `event: "acknowledged"`、`replayed: true`、`receipt` 和 `state`。新工作或配置使用新的命令标识。

`--json` 输出最终 `type: "session"` 对象，含 `event`、`session_id`、`agent`、`status`、`turns`、`response`、`error`、`config`、`usage`、`permission`、`question` 和 `duration_ms`。`--stream-json` 另含瞬时进度和持久回执信息。慢观察者可能丢失瞬时增量。最终响应来自已提交状态。Provider 用量及未知值遵循 [Session SDK](../how-it-works/sessions.md) 约定。

Prompt 首项为合法 `/skill-name` 时，CLI 加载对应 Skill，将余下文本作为用户输入。`/tmp/foo` 等路径保持普通输入。`--no-skills` 关闭此快捷方式及默认 Skill 工具。

## `kuku reply`

```text
kuku reply <session-id> <request-id> [--agent <address>] (--allow | --deny | --answer <text>)
```

回复必须匹配保留的请求，重开后同样如此。`--allow` 只授权绑定动作一次；`--deny` 拒绝；`--answer` 回答待决问题。普通 `run` 输入不会成为授权。`--command-id` 支持安全重试；已提交回复仅返回原回执和当前状态，不恢复工作或再次作出授权决定。过期或不匹配的请求明确失败。此命令也接受 `--config`、`--workspace`、`--data-dir`，观察续接并等待清理。

回复固定输出 JSON 行，无需 `--json`：新的问题回复以 `AgentSnapshot` 结束；新的授权回复先输出 `permission_decision`，再输出快照。重复回复输出上文所述含 `receipt` 和 `state` 的 `acknowledged` Session 对象。回复不使用普通 `run` 的完成对象。

## `kuku show`

```text
kuku show <session-id> [--agent <address>] [--json] [--workspace <path>] [--data-dir <path>]
```

读取最后的持久响应。`--json` 返回 Agent 快照，包括待决问题或授权标识。其他客户端在 Session 中执行时，也可使用 `show` 和指定 Session 的 `list` 查询。

## `kuku events`

```text
kuku events [-v|-vv] [--agent <address>] [--after <id>] [--limit <n>] [--json] <session-id>
```

- `-v` 或 `-vv` 增加简要显示的细节。
- `--json` 输出完整的持久事件记录。
- `--after` 为排他的持久游标；`--limit` 默认 100，接受 1–256 条。地址过滤在此有界账本页内进行。即使所有记录被过滤，最后扫描的游标也会输出到 stderr。
- `--workspace`、`--data-dir` 指定 Session 存储位置。
- 检查命令仍接受 `--conversation` 作为 `--agent` 别名。

服务分页读取 `events.jsonl` 中已提交的事实。可观察性日志单独保存在 `$KUKU_HOME/logs/`。[文件布局](file-layout.md)

## `kuku list`

```text
kuku list [<session-id>] [--all] [--workspace <path>] [--data-dir <path>] [--verbose]
```

指定 Session 后列出稳定 Agent 地址、状态和待接纳输入数量。

## `kuku delete`

```text
kuku delete [--workspace <path>] <session-id>
```

## `kuku pending`

```text
kuku pending list
kuku pending resend COMMAND_ID [--session SESSION_ID]
kuku pending discard COMMAND_ID [--session SESSION_ID]
```

`list` 向所选服务查询回执，并从本地待确认记录中移除已确认的条目。未确认条目可以用原命令标识和内容重发，也可以从客户端丢弃。丢弃会保留服务端已经提交的工作。如果不同 Session 有相同的命令标识，用 `--session` 区分。

## `kuku config`

```text
kuku config [--config <path>] [show|validate|set|policy]
```

子命令：

| 子命令 | 语法 |
|---|---|
| show | `kuku config show` |
| validate | `kuku config validate` |
| set | `kuku config set <key> <value>` |
| policy allow | `kuku config policy allow <risk>` |
| policy deny | `kuku config policy deny <risk>` |

`config validate` 按服务创建 Session 时的规则逐档解析模型。失败时会标出对应的 tier，校验期间不会调用模型服务。

`policy allow` 和 `policy deny` 当前只输出未实现提示，不修改 `policy.md`。

## `kuku prompts`

```text
kuku prompts [show [name] | export <dir>]
```

`show` 可用名称：

- `system`
- `project-context`
- `tool-guidance`
- `runtime-context`

## `kuku agents`

```text
kuku agents [list | show <name>]
```

## `kuku skills`

```text
kuku skills [list | show <name>]
```

## `kuku server` 和 `kuku web`

```text
kuku server [--listen <addr>] [--config <path>] [--password <token>] [--max-concurrent-runs <n>]
kuku server status
kuku server restart
kuku web [--session <id> [--agent <address>]] [--print-url]
```

默认值：

- `--listen 127.0.0.1:17777`
- `--max-concurrent-runs 16`

显式启动服务需要 `--password` 或 `KUKU_SERVER_PASSWORD`。`kuku web` 会在浏览器中打开所选服务，可以把工作区、Session 和 Agent 选择传给页面。使用 `--print-url` 只打印链接。受管服务的登录链接带有短时一次性票据。

`server status` 以 JSON 输出服务信息。本地服务未运行时返回 `{"state":"stopped"}`。服务忙时，`server restart` 以状态码 1 退出，并输出含 `state: "deferred"` 和当前 `service` 状态的 JSON。重启成功时返回 `state: "restarted"` 和替换后的服务状态。

要从源码内嵌 Web 界面，安装 Node.js 24 和 npm 后在仓库根目录运行以下命令。

```bash
npm --prefix apps/web ci
npm --prefix apps/web run build
cargo run --locked -p kuku-app --features embedded-web-assets -- web
```

服务认证和请求格式见[运行服务](../guides/run-the-server.md)和 [Server API](server-api.md)。
