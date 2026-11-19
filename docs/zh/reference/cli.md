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
| Ctrl+C | 有草稿时清空，草稿为空时退出 TUI。正在执行的工作在服务端继续。弹窗中只清空当前输入框。 |
| Enter | 发送草稿。Agent 忙时在下一个安全的模型请求边界接纳，如果当前轮先结束则在下一轮接纳。 |
| Alt+Enter | 把草稿排到下一轮。 |
| Shift+Enter 或 Ctrl+J | 在草稿中换行。 |
| Esc | 关闭当前获得焦点的菜单，或打断所选执行并保留草稿。 |
| Ctrl+P、F1，或草稿为空时按 `/` | 搜索可用操作和 Skill。选中 Skill 会在草稿中加入可编辑的 `/skill:NAME` 前缀，用于下一次发送。 |
| `@` 或 Tab | 将工作区相对路径补全到草稿。 |
| `/new`、`/resume` | 打开空白草稿或查找已有 Session。 |
| Alt+← 或 Alt+→ | 直接切到上一个或下一个 Agent。`main` 排第一，其余按地址排序，到达首尾时循环。 |
| `/agents` | 搜索 Session 中的 Agent、切换接收者或新建 Agent。列表显示活动、待决请求和排队数量，并在有摘录时分别标注 `Admitted` 和 `Queued` 任务。 |
| `/back` | 返回同一 Session 中刚才查看的 Agent。重复操作可在最近查看的两个 Agent 之间切换。切换时会保留各自的草稿和阅读位置。 |
| `/queue`、`/withdraw`、`/continue` | 排到下一轮、撤回排队输入，或显式继续工作。 |
| `/queued` | 在接纳前编辑排队输入。草稿为空且有可编辑输入时，按 ↑ 也可打开选择器。 |
| `/jobs` | 列出所选 Agent 的后台任务及其 ID 和状态。选中运行中的任务可取消，选中已结束的任务可读取已有的保留输出。 |
| `/jobs cancel <id>` | 取消所选 Agent 的指定后台任务，并显示任务最终状态。 |
| F2 或 `/reply` | 回复所选 Agent 的待决问题或授权。 |
| `/pending` | 检查、重试或丢弃未确认的提交。 |
| `/model`、`/thinking` | 为所选 Agent 选择模型或思考设置。 |
| `/statusline` | 打开 **Status line settings**，预览、选择、排序并保存状态栏字段。 |
| Ctrl+O | 一起展开或收起可见的对话预览，包括工具组和 Agent 卡片。 |
| `/details` 或点击对话条目 | 打开工具组查看组内调用，打开单次调用可查看目标、参数以及已有输出或错误。在 **Transcript details** 中搜索可见条目，按 Enter 切换选中条目的展开状态。 |
| `/source` | 在当前 Agent 的对话中切换助手回复的 Markdown 预览与原文。原文视图保留围栏标记。 |
| `/tools`、`/copy` | 打开保留的工具输出，或复制最近提交的助手回复完整原文。 |
| `/save` | 在发送前给粘贴文本或当前草稿命名。编辑草稿时可通过 Ctrl+P 选择此操作。 |
| `/values` | 浏览所选 Agent 的保存值。选中一项后可选择 **Read saved value** 读取，或 **Attach to draft** 附到草稿。 |
| 在工具输出查看器中按 S | 给选中的保留输出命名。在命名提示中输入 `NAME --replace` 可更新已有名称。 |
| 鼠标左键拖动 | 高亮选区，松开时将所选文字复制到剪贴板。 |
| 鼠标滚轮、Page Up、Page Down | 滚动对话。 |
| Ctrl+End，或阅读历史时按 Alt+↑ | 回到最新输出并恢复跟随。 |
| 编辑区的 ↑、↓ | 在多行草稿内移动，到边缘后滚动对话。Ctrl+↑ 和 Ctrl+↓ 浏览已发送的提示词并返回草稿。 |
| `/server`、`/reconnect`、`/restart` | 检查所连服务、重新连接，或重启受管本地服务。 |
| 菜单中的方向键、Tab、Enter | 方向键移动高亮项，Tab 补全可编辑文本，Enter 执行选中的动作。 |
| `/web`、`/quit` | 在浏览器中打开对话或退出终端客户端。 |

在选择弹窗中点击一行可高亮该项。点击也可切换正在查看的 Agent 或 Session、打开模型 ID 输入框，或查看对话详情和保留输出。应用模型档或思考设置、决定授权、撤回输入和重启仍须按 Enter 确认。

### 阅读对话

每轮包含用户消息、工具执行和助手正文。同一轮内连续成功的只读检查默认归组，显示读取、搜索和列表等调用的数量。文件修改、运行中的调用、失败和授权请求仍单独显示。**List Agents** 汇总参与者及其活动，**Query Session** 汇总匹配的事件。可打开详情检查单次调用，或用 `/tools` 查看保留的原始结果。

翻阅较早的输出时，点击输入区旁的 **Jump to latest** 可恢复跟随。

轮次统计在回答末尾显示一次，随正文滚动。浏览历史时，当前轮次和返回入口浮在输入框上方，共用输入框的左边线和文字内边距。按可用宽度显示耗时、token、缓存命中率与工具次数。

命令卡顶部用操作的简短说明作标题，缺少说明时显示 **Run command**。卡片预览可执行程序及参数，或所选 shell 与命令文本。底边显示观察到的执行状态和已知退出码。识别出测试报告时会显示程序报告的统计。点开卡片可查看记录的参数、捕获详情和可用证据。已完成的写入和编辑使用代码块样式。写入预览以实际记录的内容为准，源码按语言高亮，Markdown 使用正文渲染。编辑显示实际增删的行，卡片底边显示写入或变更的行数。

工作区脚本卡显示已完成的宿主动作和观察到的文件变更。展开后可查看各动作结果，包括部分完成和结果未知的情况。用 `/tools` 可查看单个脚本动作的保留捕获。

发送的文字会立即出现在输入框上方的 **Pending inputs**。同一条输入会显示发送、回执、排队或失败状态，接纳后进入对话。重新打开时可找回未确认和排队的输入。用 `/queued` 编辑排队输入，或用 `/withdraw` 撤回。如果 Agent 先接纳了输入，实际接纳的文字会保留在对话中。

其他 Agent 投递的消息会显示发送者地址，例如 `From main`。开启一轮的消息也会显示这个地址。

Markdown 边输出边排版，尚未闭合的代码围栏也可阅读。接入正在生成的回复时，终端可以显示已流出文本的有界前缀。已提交的回复会替换临时预览。

打断正在流式输出的回复时，有界预览会保留在原轮次中，重新打开 Session 后也能看到。

助手回复中标为 `md` 或 `markdown` 的围栏默认预览其中的文档，包括标题、列表、表格和代码块。标签不区分大小写。内部代码按标注的语言显示。普通代码围栏按原样显示内容，支持的语言带语法高亮。

用户消息、命令、Agent、文件预览和代码块都在正文四周保留内边距，窄屏时缩小左右留白。Agent 卡片和代码块占满各自缩进层级的可用正文宽度，独立的 Agent 卡片有自己的边框，与相邻命令之间留一行空白。展开时 Details 入口保留在任务和回复详情上方。卡片顶部显示 Agent 名称。点已确认的 Agent 卡片右上角 **Open agent →** 可直接切换到它的会话视图，窄屏显示 **Open →**。创建指令在独立标签下换行，默认预览最多三行，较长指令可点 Details 阅读。任务和回复各用单行预览。同一 Agent 的创建和任务投递共用一张委派卡片，展开可检查各次操作。只涉及委派双方的状态查询收入该卡片的时间线；涉及其他 Agent 的查询另行汇总。已完成回复的思考阅读入口紧贴助手标题。

模型思考时，助手回复的第一行会显示轻微动效；有公开思考内容时，这里按终端自己的节奏逐字显示一行摘录。回答从同一行接上。点击 **View thinking** 或输入 `/notes`，可以在消息原来的位置阅读已收到的思考内容。阅读区最多显示五行，回答继续显示在下方。方向键或阅读区上的滚轮可翻阅思考，Esc 或再次点击标题可收起。复制和 Markdown 源码视图使用回复正文。工具调用前的思考可在 `/details` 中查看。

### 状态与用量

空白会话显示 kuku 标题。空间充足时，会话名和操作提示在 kuku 名称下左对齐。开始工作后，会话标题缩为一行。默认状态栏最左侧是生效模型，后面显示连接状态、所选 Agent 和工作区。手机宽度下，工作区、上下文用量和平均 TPS 移到第二行。窗口很矮时底栏收为一行，为草稿留出空间。点击草稿文字可将光标移到对应字的位置，自动折行后也可直接点击。耗时紧跟当前活动名称。

输入区上方的标记和文字显示所选 Agent 的状态。执行期间标记会变化，准备就绪或工作结束后静止。界面会平滑处理短暂的阶段变化，立即显示错误和待用户决定的事项。执行中每秒更新轮次耗时，切换阶段或 Agent 时沿用原起点。结束后，耗时归到轮次总结，已接纳输入的回执从输入区退场。可用 `/statusline` 将可选的 `execution` 字段加入底栏。输入区在草稿上方显示 `To <agent>` 收件人，并随输入伸展。`Draft` 表示本地文本，`Awaiting receipt`、`Input queued` 和 `Input accepted` 分别表示等待回执、已排队和已接纳。

`Waiting for model` 表示模型请求正在等待。模型服务的思考、可见回复和工具参数陆续到达时，可在 `Thinking`、`Streaming reply` 和 `Writing tool arguments` 之间切换。`Running tools` 表示工具已经派发。`Waiting` 表示有待答问题或授权，`Failed` 表示执行错误。`Offline` 表示服务连接出现问题，`Updates unavailable` 表示实时对话订阅失败。重连或收不到更新时，`Last seen` 标明上次观察到的执行状态。

在 `/statusline` 中用 ↑ 或 ↓ 选择字段，空格切换显示，← 或 → 移动已启用字段。按 R 预览默认布局，按 Enter 保存，按 Esc 返回之前的布局。保存后立即应用布局，并写入本地 `tui.toml` 供以后启动使用。如果设置了 `KUKU_TUI_STATUSLINE`，关闭编辑器后恢复环境变量指定的布局。[终端配置](config.md#终端偏好)

| 字段 | 读数含义 |
|---|---|
| `Context …% used` | 最近一次已测请求的输入 token 数除以该次请求使用的上下文容量，反映最近一次实际发送的提示词占用。 |
| `in`、`out` | 当前 Agent 历次请求的累计输入和输出 token。输入包含缓存部分。`≥` 表示仅部分请求有读数。 |
| `Cache hit …%` | 对当前 Agent 同时报告两项计数的请求，累计缓存读取 token 占累计输入 token 的比例。`0%` 表示实测零命中，没有测得大于零的输入总量时显示横杠。 |
| `TPS` | 当前 Agent 已完成且有输出与有效计时的请求，其总输出 token 数除以总请求耗时。耗时含首个 token 等待，工具执行与重试退避均不计入。 |

`—` 表示当前没有可用读数。新请求准备好后，Context 等待该次请求上报用量，TPS 继续显示已有平均值。如只需身份、路径和用量，可在启动前设置 `KUKU_TUI_STATUSLINE=agent,workspace,context,tokens_in,tokens_out,cache,tps`。

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
| `--show-thinking` | 显示模型服务提供的公开思考文字或摘要 |
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
