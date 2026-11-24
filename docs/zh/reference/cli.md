# CLI

## 入口模式

- `kuku` 在交互终端打开全屏客户端（TUI）。
- `kuku run ...` 执行非交互任务，适用于脚本或无 TTY 环境。

TUI 和 `kuku run` 都通过 kuku 服务提交任务。本地客户端复用所选数据目录的受管服务。[运行服务](../guides/run-the-server.md)

直接运行 `kuku`，交互配置第一个连接和模型。无头配置可用 [`kuku config set --patch`](#kuku-config)。[配置 kuku](../start/configuration.md)

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
| `kuku run ...` | 提交文本或提示词文件并观察执行 |
| `kuku reply <session-id> <request-id>` | 回复持久问题或授权请求 |
| `kuku show <session-id>` | 查看 Session 的最终输出 |
| `kuku events <session-id>` | 查看持久 Session 事件 |
| `kuku list` | 列出 Session |
| `kuku pending ...` | 核对丢失回执的提交 |
| `kuku delete <session-id>` | 删除 Session |
| `kuku config ...` | 查看、验证或修改配置 |
| `kuku prompts ...` | 查看或导出 Prompt 资产 |
| `kuku agents ...` | 列出或查看 Agent 预设 |
| `kuku skills ...` | 列出或查看 Skill |
| `kuku server ...` | 显式启动 HTTP 服务 |
| `kuku server status` | 查看服务身份与当前活动 |
| `kuku server restart` | 空闲时重启受管本地服务 |

## `kuku auth`

| 命令 | 用途 |
|---|---|
| `kuku auth login chatgpt [名称]` | 添加账号或重新授权已保存的账号 |
| `kuku auth list` | 查看账号名称、脱敏邮箱和登录状态 |
| `kuku auth models chatgpt <名称>` | 列出该账号可用的模型 |
| `kuku auth proxy chatgpt <名称> (--proxy URL \| --proxy-from-env \| --no-proxy)` | 修改已保存账号的网络模式 |
| `kuku auth logout chatgpt <名称>` | 清除该账号的令牌并尝试远程撤销 |

执行 `login` 时省略网络选项，则已保存的账号沿用原设置，新账号跟随环境。代理需要认证时，传入 `--proxy-user` 和 `--proxy-password-env` 或 `--proxy-password-file` 之一。密码从引用的来源读取。[无头登录流程](../start/configuration.md#在无头主机登录订阅)

## 终端界面

### 发送、编辑与停止

| 操作 | 方式 |
|---|---|
| 立即发送 | 按 `Enter`。Agent 正在工作时，可能在当前 Turn 收到这条消息。 |
| 排队到下一轮 | 按 `Alt+Enter` 放进 **Pending inputs** 队列。 |
| 编辑排队消息 | 点击消息，或在空白输入框按 `↑` 选中；`Enter` 编辑，`Delete` 移出队列。 |
| 停止当前 Turn | Agent 运行时按 `Esc`，已显示的内容会保留。 |

在输入框使用终端的常规粘贴快捷键。终端转发 `Ctrl+V` 或 `Ctrl+Shift+V` 按键时，kuku 会读取本地剪贴板。接收到空的括号粘贴事件时，kuku 也会尝试读取图片。如果终端自行处理快捷键且只转发非空文本，可以用 `Ctrl+Alt+V` 直接触发剪贴板读取。图片在草稿中显示为有颜色的 `[Image #1]`。光标会一次跨过整个标记，在标记旁按退格键或 Delete 可以移除整张附件。文本仍可使用终端的常规粘贴快捷键。粘贴一个可读取的 PNG 或 JPEG 文件路径也会添加图片附件。在 WSL 中，kuku 会将 Windows 盘符路径解析到已挂载的磁盘。输入框上方会显示图片张数，以及是否使用 PNG 保留透明度。

按 `Enter` 后，草稿上方出现画质选择。默认高亮 **压缩**，三秒无操作就按默认发送。压缩模式用质量 85 的 JPEG，含透明像素的图片用 PNG。用 `Tab`、方向键或鼠标选择 **原图** 可以保留源尺寸和像素。操作面板会取消倒计时。按 `Enter` 确认发送，或按 `Esc` 保留草稿。每条消息最多四张，每张提交图片上限 1 MiB。

图片随草稿一起发送。当前模型无法接收图片时，kuku 会保留草稿，可以切换模型或移除附件后再发送。

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
| `Ctrl+P` 或 `F1` | 查看快捷键和用法帮助入口，保留当前草稿。 |
| 空草稿按 `/`，或 `Ctrl+P` 后按 `/` | 搜索斜杠命令和 Skill；选中 Skill 会填入可编辑的 `/skill:NAME` 前缀。 |
| `@` 或 `Tab` | 补全工作区相对路径。 |
| `Alt+←` / `Alt+→` | 在 Agent 间循环切换（`main` 排第一，其余按地址排序）。 |
| `F2` | 有待审批工具时打开审批面板，否则使用当前问题、恢复等可见交互控件。 |
| `F3` | 打开工作区权限模式菜单。 |
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
| `/model` | `/models` | Enter 为当前 Agent 选择高亮的连接和模型。`d` 打开设为新 Session 默认的确认页，`e` 编辑，`/` 搜索。`Ctrl+L` 打开同一个列表。 |
| `/speed [档位]` | `/fast`、`/ultra`、`/ultrafast` | 在会话输入框上方选择当前 Agent 的服务档位。快捷命令选择 `fast` 或 `ultrafast`。选择 **Follow model default** 清除覆盖设置。输入可筛选已有档位，按 Tab 补全选中的档位。添加未知档位时可以选择是否探测。 |
| `/thinking` | `/think`、`/effort` | 设置当前 Agent 思考等级，选 **Use default** 清除覆盖，或在可写本地路由上按 `D` + `Enter` 设为模型默认。 |
| `/statusline` | `/status-line` | 预览、开关、排序并保存状态栏字段到 `tui.toml`。 |
| `/permissions [full\|ask\|read-only]` | `/perm`、`/permission` | 打开权限菜单或直接设定模式。 |
| `/notifications` | `/notify`、`/sound` | 开关并保存终端声音设置。 |
| `/save` | `/save-value` | 发送前为粘贴内容或草稿命名保存。 |
| `/values` | `/value`、`/saved-values` | 浏览保存值，读取或附到草稿。 |
| `/server` | `/service` | 查看已连接的服务（`Ctrl+R` 重连）。 |
| `/restart` | `/restart-service` | 确认重启受管本地服务（有任务运行时会延后）。 |
| `/help` | `/hotkeys` | 查看键盘快捷键和用法帮助入口。 |
| `/quit` | `/exit`、`/q` | 退出 TUI（已接纳任务在服务端继续）。 |

### 阅读对话

- **工具活动**：连续的工作区调用归在一组。概览显示读取和搜索次数、诊断及每项已记录的文件变更，有内容时附上简短预览。重复诊断只显示一次并附次数。选择组标题查看各项调用，也可以选择文件行原位展开。减号表示原文件中的行，加号表示新文件中的行。顶部的 **Messages** 可查看单次调用及其完整输出。脚本展开后依次显示 **Input**、**Actions** 和 **Output**。输入预览 Agent 提交的 JavaScript，操作区显示调用记录和完成计数。对象输出先显示字段总数和最多三个字段的名称与值摘要。选择 **Input** 或 **Output** 即可在对应位置展开。
- **工具结果**：命令结果显示命令、可选标题、状态、退出码及识别出的测试统计。展开命令后，选择 **View output** 打开这条命令保存的输出，`run_js` 内的命令也一样。选择 **Read saved output** 可打开这次读取记录的字节范围。展开文件行可查看写入内容或 diff。展开 `run_js` 结果可查看已完成的宿主操作、已知的失败原因和保留输出。压缩标题显示已记录的耗时及摘要前后的估算输入 token。
- **委派与队列**：其他 Agent 投递的消息标注发送者（如 `From main`）；同一 Agent 的创建与任务投递合并在委派卡内，带 **Open agent →** 快捷入口和可展开详情。已发未处理消息显示在 **Pending inputs**。
- **流式与 Markdown**：边输出边排版（支持未闭合围栏、任务列表、嵌套引用/列表、行内 `<sup>`/`<sub>`/`<br>`、可点击 HTTP/HTTPS 链接以及默认渲染的 `md`/`markdown` 围栏）。打断流式输出时会在原 Turn 保留有界预览。
- **思考过程**：模型思考时在回复顶部流式显示摘录。点击 **View thinking**（或在 **Messages** 里选 **Thinking**）可就地展开 5 行可滚动阅读区；工具调用前的思考也在 **Messages** 中查看。
- **跳转与统计**：阅读历史时点 **Jump to latest** 恢复跟随；每轮回复末尾汇总耗时、token、缓存命中率与工具次数。

### 状态与用量

底部状态栏默认显示生效模型、思考等级和已配置的服务档位，例如 `gpt-6.2-sol xhigh ultrafast`、连接提示、当前 Agent、工作区、上下文用量、缓存命中率、平均 TPS、累计输入和输出 token 以及请求费用。待生效的设置显示在 **Next model request**。

如果下一条消息的模型路由或生效服务档位与该 Agent 上一次请求不同，输入框上方会显示变化并提醒缓存可能无法复用。再按一次 Enter，按原先的时机发送。Esc 收起提示并保留草稿。

输入区上方显示收件人（`To <agent>`）、输入投递状态（`Draft`、`Awaiting receipt`、`Input queued`、`Input accepted`）、当前 Turn 实时耗时及执行状态：

- `Waiting for model`、`Thinking`、`Streaming reply`、`Writing tool arguments`、`Running tools`
- `Waiting`（等待答复或授权）、`Failed`（执行出错）
- `Offline`（连接异常）、`Updates unavailable`（实时流订阅失败），此时以 `Last seen` 标明上次观察状态

可用 `/statusline` 交互编辑字段（写入 `tui.toml`），或用 `KUKU_TUI_STATUSLINE` 做单次覆盖。[终端偏好](config.md#终端偏好)

| 字段 | 读数含义 |
|---|---|
| `Context` | 最近一次已测请求的输入 token 数除以该次请求的上下文窗口。默认布局在空间充足时同时显示输入 token 数和窗口上限。 |
| `in`、`out` | 当前 Agent 历次请求的累计输入（含缓存）与输出 token。`≥` 表示仅部分请求上报了读数。 |
| `Cache hit …%` | 同时上报两项计数的请求中，累计缓存读取 token 占累计输入 token 的比例。`0%` 为实测零命中，无正输入读数时显示 `—`。 |
| `TPS` | 已完成且有输出与有效计时的请求总输出 token 除以总请求耗时（含首 token 等待，不含工具执行与重试退避）。 |
| 请求费用 | Session 请求费用合计，单位为美元。`~` 表示合计包含估算，`≥` 表示费用下界。 |

## `kuku run`

```text
kuku run [OPTIONS] <PROMPT...>
kuku run [OPTIONS] --prompt-file <PATH|->
```

| 选项 | 含义 |
|---|---|
| `--prompt-file <PATH\|->` | 读取完整的 UTF-8 提示词，用 `-` 从标准输入读取。只能选一种输入来源。 |
| `-s`、`--session <id>` | 继续已有 Session |
| `-c`、`--continue` | 继续最近的 Session |
| `--agent <address>` | 指定目标 Agent，默认 `main` |
| `--preset <name>` | 在新 Session 中按预设创建 Agent |
| `--connection <name>` 与 `--model <id>` | 按精确 ID 选择已配置模型，两个选项同时传入 |
| `--reasoning-effort <value>` | 覆盖选中模型的原生思考强度 |
| `--service-tier <value>` | 覆盖 Provider 原生服务档位 |
| `--max-output-tokens <n>` | 每次模型请求的输出 token 上限 |
| `--command-id <id>` | 为本次提交指定稳定 ID，以便原样重试 |
| `--json` | 向标准输出写入一个最终 JSON 结果 |
| `--stream-json` | 向标准输出写入逐行 JSON 事件，末行为最终结果 |
| `--raw` / `--verbose` | 选择纯文本输出或详细的可读输出 |
| `--show-thinking` | 观察运行时展示已回报的思考内容 |
| `--prompts-dir <path>` | 覆盖随附的 Prompt 资产 |
| `--no-agents` / `--no-skills` | 关闭委派工具或关闭 Skill 工具与加载 |

从文件或标准输入读取的内容会按文本传入，包括其中的换行。若要[显式调用 Skill](../guides/use-skills.md#运行时使用-skill)，请使用位置参数传入提示词。

最终结果包含 `type: "session"`、表示结果的 `status` 和 `event`，以及可用的 `session_id` 和 `agent`。运行完成后返回最终 `response`，运行失败时返回 `error`。已知的实际连接、模型和用量分别见 `connection`、`model` 和 `usage`。等待时，`question` 或 `permission` 中的 `id` 标识待答复的请求。诊断写往 stderr。参数无效时可能在生成 Session 结果前退出。

回执标识本次提交的输入。Agent 接纳输入后，即使另一客户端开启了后续 Turn，流式输出和最终结果也只来自接纳该输入的 Turn。结果会在用量信息可用时返回该 Turn 的用量。排队中的输入被撤回或替换后，本次运行结束，返回 `status: "cancelled"` 和 `input_status: "withdrawn"`。

用已提交的 `--command-id` 重试时，返回原始 `receipt`、`replayed: true` 和已知的当前 `input_status`。用 `kuku show <SESSION_ID> --json` 查看 Agent 的当前状态。

| 退出码 | 结果状态 | 含义 |
|---|---|---|
| `0` | `completed` | 运行完成 |
| `0` | `acknowledged` | 已有持久提交回执 |
| `3` | `waiting` | Agent 等待答复或授权 |
| `1` | `failed` | 运行失败 |
| `130` | `cancelled` | 运行已取消 |
| `2` | | 命令行参数无效 |

[运行任务](../guides/run-a-task.md)

## `kuku reply`

```text
kuku reply <SESSION_ID> <REQUEST_ID> --allow
kuku reply <SESSION_ID> <REQUEST_ID> --deny
kuku reply <SESSION_ID> <REQUEST_ID> --answer "<text>"
```

答复持久授权请求或问题，并观察后续运行。要回复 `main` 之外的 Agent，请传 `--agent <address>`。可用 `--command-id <id>` 原样重试回复。首次答复返回接纳答案或恢复授权操作的 Turn 结果。答复后台脚本的授权请求会返回对应 Job 的结果。Ctrl-C 针对本次答复的后续执行。原样重试时，返回原回执及 `state` 中的当前 Agent 快照。

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
kuku list [-a | --all] [--json]
kuku list <SESSION_ID> [--json]
```

列出所选工作区的 Session，也可用 `--all` 查看所有工作区。传入 Session ID 可列出其中的 Agent。

## `kuku pending`

```text
kuku pending list
kuku pending resend <COMMAND_ID> [--session <id>]
kuku pending discard <COMMAND_ID> [--session <id>]
```

查看因超时或断连而丢失回执的提交。`list` 查询回执。提交仍未确认时，用 `resend` 原样重试，或用 `discard` 移除本地待确认记录。

## `kuku delete`

```text
kuku delete <SESSION_ID>
```

删除 Session，必要时先关闭。执行、清理或 Job 尚未结束时无法删除。

## `kuku config`

```text
kuku config show [--json]
kuku config models [--discover --connection NAME]
kuku config validate
kuku config set <KEY> <VALUE>
kuku config set --patch <TOML>
```

四个命令按同一顺序选择本地配置：

1. `--config PATH` 指定的文件。
2. 该数据目录下正在运行的受管服务所用的配置文件。
3. 数据目录下的 `config.toml`。

`show` 将脱敏 TOML 写到 stdout，将路径写到 stderr。加 `--json` 可在 JSON 结果的 `config` 字段中读取 TOML。其他命令默认输出 JSON，包含 `path`、`status`、`diagnostics`（适用时带字段名）和 `service.state`。校验成功时状态为 `valid`，写入成功时为 `updated`。

`validate` 本地检查文件和模型设置，无需发出 Provider 请求。通过 `service.state` 判断本文件版本是否已加载（`loaded`）、有待重载（`pending_reload`）、指向不同路径（`different_path`）、服务已停止（`stopped`）或状态未知（`unknown`）。服务停止时也能运行配置命令。再次校验可核对已加载的版本。[配置 kuku](../start/configuration.md)

`models` 列出已配置的连接和模型，每项包含 `selection` 或 `error`。加上 `--discover --connection NAME` 后查询指定连接的模型列表，在 `models[].id` 中返回精确 ID。

`set KEY VALUE` 可修改一个标量。TOML 路径含带引号的模型 ID 时，请给整个路径加引号，例如 `kuku config set 'model.work."your-model-id".max_output_tokens' 8192`。要提交完整改动或创建缺失文件，用 `--patch` 将 TOML 文本作为一个参数传入。

可选的顶层 `delete` 数组指定应用补丁前要删除的 TOML 路径。例如，可同时删除一条模型注册并将默认值改为另一个已配置模型。

```toml
delete = ['model.work."your-model-id"']
default_model = { connection = "work", model = "another-model-id" }
```

用 `provider.NAME` 删除连接时，在同一补丁中一并删除或替换依赖它的模型注册。补丁合并嵌套表，并整体替换 `default_model`。命令校验完整结果后才写入。补丁无效或并发写入冲突时返回非零退出码和诊断。[最小配置](config.md#最小配置) · [从 shell 配置](../guides/run-a-task.md#从-shell-配置)

## `kuku prompts`

```text
kuku prompts
kuku prompts show [NAME]
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

## `kuku server`

```text
kuku server [--listen <addr:port>] [--password <secret>] [--max-concurrent-runs <n>]
kuku server status
kuku server restart [--config PATH]
```

- `kuku server` 需指定 `--password` 或 `KUKU_SERVER_PASSWORD`。
- `kuku server status` 返回 JSON 格式的服务状态（本地服务未运行时为 `{"state":"stopped"}`）。
- `kuku server restart` 忙碌时退出码为 `1` 并输出 `state: "deferred"`，重启成功输出 `state: "restarted"`。

[运行服务](../guides/run-the-server.md) · [HTTP 服务 API](server-api.md)
