# CLI

## 入口模式

- `kuku` 在交互终端打开全屏客户端（TUI）。
- `kuku run ...` 执行非交互任务，适用于脚本或无 TTY 环境。

TUI、`kuku run` 和浏览器界面都通过 kuku 服务提交任务。本地客户端复用所选数据目录的受管服务。[运行服务](../guides/run-the-server.md)

首次运行向导与 `kuku init` 共用本地 Provider 配置流程。`kuku init` 需要 TTY，可编辑有效的已有配置，不会启动服务，也不配置远程服务。[配置 kuku](../start/configuration.md)

## 连接与工作区选项

| 选项 | 含义 |
|---|---|
| `-w`、`--workspace <path>` | 命令和文件工具的工作区。本地默认用解析后的当前目录；远程 Session 需指定服务端机器上的绝对路径。 |
| `--data-dir <path>` | 本地服务的数据目录（默认 `$KUKU_HOME` 或 `~/.kuku`）。搭配 `--remote` 时用于客户端本地存储目录。 |
| `--config <path>` | 指定本地配置文件。远程配置在服务端机器上管理。 |
| `--remote <url>` | 显式连接远程服务地址。 |
| `--token <token>` | 显式连接的服务访问令牌，也可通过 `KUKU_TOKEN` 设置。 |

使用 `kuku run` 时把选项放在提示词前面，其余参数会拼作提示词。

## 顶层命令

| 命令 | 用途 |
|---|---|
| `kuku run <prompt...>` | 提交任务并观察执行 |
| `kuku reply <session-id> <request-id>` | 回复持久问题或授权请求 |
| `kuku show <session-id>` | 查看 Session 的最终输出 |
| `kuku events <session-id>` | 查看持久 Session 事件 |
| `kuku list` | 列出 Session |
| `kuku pending ...` | 核对丢失回执的提交 |
| `kuku delete <session-id>` | 删除 Session |
| `kuku config ...` | 查看、验证或修改配置 |
| `kuku init` | 打开本地 Provider 与凭据配置向导 |
| `kuku prompts ...` | 查看或导出 Prompt 资产 |
| `kuku agents ...` | 列出或查看 Agent 预设 |
| `kuku skills ...` | 列出或查看 Skill |
| `kuku server ...` | 显式启动 HTTP 服务 |
| `kuku server status` | 查看服务身份与当前活动 |
| `kuku server restart` | 空闲时重启受管本地服务 |
| `kuku web ...` | 打开所选服务的浏览器界面 |

## 终端界面

### 发送、编辑与停止

| 操作 | 方式 |
|---|---|
| 立即发送 | 按 `Enter`。Agent 正在工作时，可能在当前 Turn 收到这条消息。 |
| 排队到下一轮 | 按 `Alt+Enter` 放进 **Pending inputs** 队列。 |
| 编辑排队消息 | 点击消息，或在空白输入框按 `↑` 选中；`Enter` 编辑，`Delete` 移出队列。 |
| 停止当前 Turn | Agent 运行时按 `Esc`，已显示的内容会保留。 |

### 回退与分支切换

Agent 空闲时按 `Ctrl+G` 或输入 `/rewind`（或 `/jump`）打开对话历史。当前主线和已保存分支按时间排列，默认选中最新一轮，向上翻可加载更早记录。

选中已结束的 Turn 按两次 `Enter` 即可切回该轮结束时的对话（保留回答和工具结果）。输入框为空时会自动带入原消息作为草稿，已有草稿不受影响。在列表中选中标有 **Saved** 的条目可切回先前分支。

默认保留当前工作区文件。往回切到当前主线的较早 Turn 时，可在确认页按 `Space` 预览可恢复文件与增删行数，再按 `Enter` 一并切换对话并恢复文件。[消息回退机制](../how-it-works/sessions.md#消息回退)

### 常用快捷键与控件

| 按键或控件 | 行为 |
|---|---|
| `Ctrl+C` | 有草稿时清空草稿；草稿为空时退出 TUI（服务端工作继续）。在编辑弹窗中只清空当前输入框。 |
| `Shift+Enter` 或 `Ctrl+J` | 草稿中换行。 |
| `Esc` | 关闭弹窗、退出聚焦控件，或运行/清理期间打断执行。关闭答复编辑器保留草稿；空闲且草稿为空时连按两次 `Esc` 打开对话历史。 |
| `Ctrl+P` 或 `F1` | 打开只读快捷键帮助，保留当前草稿。 |
| 空草稿按 `/`，或 `Ctrl+P` 后按 `/` | 搜索斜杠命令和 Skill；选中 Skill 会填入可编辑的 `/skill:NAME` 前缀。 |
| `@` 或 `Tab` | 补全工作区相对路径。 |
| `Alt+←` / `Alt+→` | 在 Agent 间循环切换（`main` 排第一，其余按地址排序）。 |
| 编辑器外按 `F2` | 依次聚焦可见交互控件（待决请求、认证修复、继续/停止、待确认投递恢复、上一个 Agent、服务操作、**Messages**）。`F2` 或点击问题卡片打开答复编辑器；`F2` 或点击权限卡片聚焦请求，再用 `←`/`→` + `Enter` 选择 **Deny** / **Allow once**（键盘拒绝路径为 `F2`、`←`、`Enter`）。`Ctrl+R` 重连。 |
| `Ctrl+T` | 切换当前模型支持的思考等级，在下一个安全请求边界生效。 |
| **Repair credentials** | 出现匹配的认证失败后显示，用于更新受管本地连接并明确重启其服务；远程连接显示服务端修复步骤。[认证失败后的恢复](../start/configuration.md#认证失败后的恢复) |
| **Messages**（窄屏 **Msg**） | 浏览当前 Agent 的用户、助手和工具条目并执行操作（`Full output` 打开保留输出或完整内联内容；多输出 JS 脚本可进一步选择单个动作）。 |
| `Ctrl+O` | 一起展开或收起可见的对话预览（工具组和 Agent 卡片）。 |
| 在输出查看器中按 `S` | 给选中的保留输出命名（`NAME --replace` 覆盖已有名称）。 |
| 鼠标左键拖动 | 选中文字，松开即复制到剪贴板。 |
| 滚轮、`Page Up` / `Page Down` | 滚动对话。 |
| `Ctrl+End` 或 `Alt+↑` | 跳回最新输出并恢复自动跟随。 |
| 编辑区 `↑` / `↓` | 多行草稿内移动，到达边缘后滚动对话。`Ctrl+↑` / `Ctrl+↓` 浏览已发送历史（或队列聚焦时浏览排队输入）。 |
| `exit` 或 `quit` | 单行草稿只有这一个词且无附件时，发送会提示 **Exit kuku?**（`Y` 退出，`Esc` 继续编辑）。 |

弹窗中点击行可高亮；点击也可切换 Agent/Session、打开模型输入框或查看消息与输出。应用模型/思考设置、权限决定或服务重启仍需按 `Enter` 确认。

### 斜杠命令

命令与别名均可搜索，`Tab` 补全主命令名并保留参数。`/reload` 保留给配置重载。

| 命令 | 别名 | 行为 |
|---|---|---|
| `/new` | `/new-session` | 打开空白草稿，旧 Session 仍可打开。 |
| `/resume` | `/sessions` | 查找并切换当前工作区的已有 Session。 |
| `/rewind` | `/backtrack`、`/jump` | 打开对话历史并切换到选中 Turn。 |
| `/compact` | `/compress`、`/summarize` | 空闲时压缩当前 Agent 的较早上下文（支持 `/compact retry` 与 `/compact undo`）。 |
| `/agents` | `/agent`、`/subagents` | 搜索 Session 中的 Agent、切换接收者或新建 Agent。 |
| `/jobs` | `/job`、`/ps` | 列出当前 Agent 的后台 Job，读取实时输出或取消（`/jobs cancel <id>`）。 |
| `/model` | `/models` | 选择、添加或编辑模型路由（`Ctrl+L` 相同）。 |
| `/thinking` | `/think`、`/effort` | 设置当前 Agent 思考等级，选 **Use default** 清除覆盖，或在可写本地路由上按 `D` + `Enter` 设为模型默认。 |
| `/statusline` | `/status-line` | 预览、开关、排序并保存状态栏字段到 `tui.toml`。 |
| `/save` | `/save-value` | 发送前为粘贴内容或草稿命名保存。 |
| `/values` | `/value`、`/saved-values` | 浏览保存值，读取或附到草稿。 |
| `/server` | `/service` | 查看已连接的服务（`Ctrl+R` 重连）。 |
| `/restart` | `/restart-service` | 确认重启受管本地服务（有任务运行时会延后）。 |
| `/web` | `/browser` | 在浏览器中打开当前对话。 |
| `/help` | `/hotkeys` | 查看键盘快捷键。 |
| `/quit` | `/exit`、`/q` | 退出 TUI（已接纳任务在服务端继续）。 |

### 阅读对话

- **工具活动**：连续的读取、搜索和文件变动共用一条标题。文件路径和变动行数直接可见。选中标题可以查看各次读取和搜索，选中文件可展开它的差异，`-` 后是旧文件行号，`+` 后是新文件行号。运行中的调用、失败和授权请求单独显示。标题栏的 **Messages** 可查看各次调用和完整输出。
- **工具卡片**：命令卡显示命令、可选标题、状态、退出码及识别出的测试统计。展开文件卡可以查看写入内容或 diff。`run_js` 卡片列出已完成的宿主操作、已知的失败原因和保留输出。压缩卡片显示摘要前后的估算输入 token。
- **委派与队列**：其他 Agent 投递的消息标注发送者（如 `From main`）；同一 Agent 的创建与任务投递合并在委派卡内，带 **Open agent →** 快捷入口和可展开详情。已发未处理消息显示在 **Pending inputs**。
- **流式与 Markdown**：边输出边排版（支持未闭合围栏、任务列表、嵌套引用/列表、行内 `<sup>`/`<sub>`/`<br>`、可点击 HTTP/HTTPS 链接以及默认渲染的 `md`/`markdown` 围栏）。打断流式输出时会在原 Turn 保留有界预览。
- **思考过程**：模型思考时在回复顶部流式显示摘录。点击 **View thinking**（或在 **Messages** 里选 **Thinking**）可就地展开 5 行可滚动阅读区；工具调用前的思考也在 **Messages** 中查看。
- **跳转与统计**：阅读历史时点 **Jump to latest** 恢复跟随；每轮回复末尾汇总耗时、token、缓存命中率与工具次数。

### 状态与用量

底部状态栏默认显示生效模型与思考等级（如 `gemini-4-neon · High`；`Default` 表示由 Provider 自行决定）、连接状态、当前 Agent、工作区、上下文用量与平均 TPS。待生效的模型/思考设置显示在 **Next model request**。

输入区上方显示收件人（`To <agent>`）、输入投递状态（`Draft`、`Awaiting receipt`、`Input queued`、`Input accepted`）、当前 Turn 实时耗时及执行状态：

- `Waiting for model`、`Thinking`、`Streaming reply`、`Writing tool arguments`、`Running tools`
- `Waiting`（等待答复或授权）、`Failed`（执行出错）
- `Offline`（连接异常）、`Updates unavailable`（实时流订阅失败），此时以 `Last seen` 标明上次观察状态

可用 `/statusline` 交互编辑字段（写入 `tui.toml`），或用 `KUKU_TUI_STATUSLINE` 做单次覆盖。[终端偏好](config.md#终端偏好)

| 字段 | 读数含义 |
|---|---|
| `Context …% used` | 最近一次已测请求的输入 token 数除以该次请求的上下文窗口。 |
| `in`、`out` | 当前 Agent 历次请求的累计输入（含缓存）与输出 token。`≥` 表示仅部分请求上报了读数。 |
| `Cache hit …%` | 同时上报两项计数的请求中，累计缓存读取 token 占累计输入 token 的比例。`0%` 为实测零命中，无正输入读数时显示 `—`。 |
| `TPS` | 已完成且有输出与有效计时的请求总输出 token 除以总请求耗时（含首 token 等待，不含工具执行与重试退避）。 |

## `kuku run`

```text
kuku run [OPTIONS] <PROMPT...>
```

| 选项 | 含义 |
|---|---|
| `-s`、`--session <id>` | 继续已有 Session |
| `-a`、`--agent <name>` | 指定目标 Agent（默认 `main`） |
| `-m`、`--model <spec>` | 为本次运行覆盖模型路由 |
| `--json` | 每行输出一条 JSON |
| `-o`、`--output <path>` | 将最终输出写入文件 |
| `--no-wait` | 服务接纳输入后立即返回 |
| `--auto-approve` | 本次运行自动批准权限请求 |
| `--wait-for-reply` | 有持久请求等待外部回复时保持连接 |
| `--timeout <seconds>` | 等待超时后退出（不取消服务端工作） |

`--json` 模式按行输出 `session`、`accepted`、`delta`、`tool.call`、`tool.result`、`request`、`turn.completed`、`turn.failed` 和 `error`。等待超时退出码为 `124`。[运行任务](../guides/run-a-task.md)

## `kuku reply`

```text
kuku reply <SESSION_ID> <REQUEST_ID> --allow
kuku reply <SESSION_ID> <REQUEST_ID> --deny
kuku reply <SESSION_ID> <REQUEST_ID> --answer "<text>"
kuku reply <SESSION_ID> <REQUEST_ID> --answer-file <path>
kuku reply <SESSION_ID> <REQUEST_ID> --answer-stdin
```

答复持久授权请求或问题。同一回复已落盘时幂等成功返回。

## `kuku show`

```text
kuku show <SESSION_ID> [--agent <name>] [--json]
```

打印 Session 或指定 Agent 的最终回复。

## `kuku events`

```text
kuku events <SESSION_ID> [--agent <name>] [--kind <kind>] [--after <id> | --before <id>] [--limit <n>] [--all] [--json]
kuku events --file <EVENTS_JSONL> [--agent <name>] [--kind <kind>] [--after <id> | --before <id>] [--limit <n>] [--all] [--json]
```

查看持久 Session 事件。`--json` 每行输出一条记录到 `stdout`，翻页参数输出到 `stderr`。`--file` 离线读取本地日志（对已封存历史与开放尾部透明）。[Session 事件](events.md)

## `kuku list`

```text
kuku list [--json]
```

列出所选工作区的 Session。

## `kuku pending`

```text
kuku pending list [--json]
kuku pending reconcile [--id <record-id>] [--json]
kuku pending drop <record-id> [--json]
```

查看并核对因超时或断连而丢失回执的提交。

## `kuku delete`

```text
kuku delete <SESSION_ID> [--force]
```

删除指定 Session。

## `kuku config`

```text
kuku config path
kuku config show
kuku config validate
kuku config set <key> <value>
kuku config init
```

`kuku config show` 会遮盖 API 密钥。`kuku config set` 修改标量配置项（如 `default_model`、`provider.<name>.model_pricing.<model>.mode`、`provider.<name>.proxy.mode`）。`kuku config init` 写入初始模板（非交互）；交互式 Provider 与凭据配置请用 `kuku init`。

## `kuku prompts`

```text
kuku prompts list [--json]
kuku prompts show <name> [--json]
kuku prompts export <dir>
```

查看或导出内置 Prompt 资产。

## `kuku agents` 与 `kuku skills`

```text
kuku agents list
kuku agents show <name>
kuku skills list
kuku skills show <name>
```

列出和查看已发现的 Agent 预设与 Skill。

## `kuku server` 与 `kuku web`

```text
kuku server [--host <addr>] [--port <port>] [--password <secret>]
kuku server status
kuku server restart
kuku web [--session <id>] [--agent <name>] [--print-url]
```

- `kuku server` 需指定 `--password` 或 `KUKU_SERVER_PASSWORD`。
- `kuku web` 打开所选服务的浏览器界面（受管登录链接带短期一次性 ticket；`--print-url` 仅打印链接）。
- `kuku server status` 返回 JSON 格式的服务状态（本地服务未运行时为 `{"state":"stopped"}`）。
- `kuku server restart` 忙碌时退出码为 `1` 并输出 `state: "deferred"`，重启成功输出 `state: "restarted"`。

从源码仓库构建内置 Web 界面（需要 Node.js 24 与 npm）：

```bash
npm --prefix apps/web ci
npm --prefix apps/web run build
cargo run --locked -p kuku-app --features embedded-web-assets -- web
```

[运行服务](../guides/run-the-server.md) · [HTTP 服务 API](server-api.md)
