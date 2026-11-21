# CLI

## 入口模式

- `kuku` 在交互终端打开全屏客户端。
- `kuku run ...` 执行非交互任务，适用于脚本或没有交互终端的环境。

终端、脚本和浏览器客户端都向服务提交任务。本地客户端复用所选数据目录的受管服务。[运行服务](../guides/run-the-server.md)

首次运行交互设置与 `kuku init` 共用本地 provider 设置流程。`kuku init` 需要 TTY，可编辑有效的已有配置，并且不会为了配置而启动服务。不支持远程配置。参见[配置指南](../start/configuration.md)。

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
| `kuku init` | 打开共享的本地 provider 和凭据设置 |
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
| Ctrl+C | 有草稿时清空，草稿为空时退出 TUI。正在执行的工作在服务端继续。编辑对话框中只清空当前输入框。 |
| Enter | 发送草稿。Agent 忙时在下一个安全的模型请求边界接纳，如果当前轮先结束则在下一轮接纳。 |
| Alt+Enter | 把草稿排到下一轮。 |
| Shift+Enter 或 Ctrl+J | 在草稿中换行。 |
| Esc | 关闭面板或弹窗，或从确认回到上一步。关闭答复编辑器会按请求 ID 保存答复草稿。在主界面，即使上下文、队列或请求有焦点，Running 或清理阶段也会被打断。空闲且输入框为空时连按 Esc 两次打开消息回退。 |
| Ctrl+P 或 F1 | 显示只读快捷键帮助并保留草稿；在帮助中按 Enter 或点击不会执行操作。 |
| 草稿为空时按 `/`，或先按 Ctrl+P 再按 `/` | 在输入框上方搜索操作和 Skill。选中 Skill 会在草稿中加入可编辑的 `/skill:NAME` 前缀，用于下一次发送。 |
| `@` 或 Tab | 将工作区相对路径补全到草稿。 |
| Alt+← 或 Alt+→ | 直接切到上一个或下一个 Agent。`main` 排第一，其余按地址排序，到达首尾时循环。 |
| 草稿为空时按 ↑，或点击 Pending inputs 中可见的可编辑排队行 | 在内联队列中选中该条输入。按 ↑/↓ 移动，Enter 编辑，Delete 撤回；空闲或 Waiting 时按 Esc 返回输入框，Running 或清理时按 Esc 会打断。 |
| 编辑器外按 F2 | 按顺序循环可见控件：请求、可用的认证修复、继续或停止、待确认投递恢复、上一个 Agent、服务操作，最后是 Messages。F2 或点击问题卡片会直接打开答复编辑器；Esc 按请求 ID 保存未提交答案，并保留主草稿。F2 或点击权限卡片会聚焦请求，供用户明确选择。Enter 执行已聚焦控件；点击其他有副作用的控件只聚焦。Running 或清理时 Esc 无论焦点为何都会打断；否则退出焦点。Ctrl+R 仍会重连。 |
| 权限请求卡片 | 用 ←/→ 或点击明确选择 **Deny** 或 **Allow once**，再按 Enter。键盘拒绝路径是 F2、←、Enter。Waiting 时 Esc 离开焦点，不作答也不打断。 |
| Ctrl+T | 循环切换所选模型支持的思考等级，对当前 Agent 在下一个安全请求边界生效，并保留草稿与阅读位置。 |
| 认证修复 | 匹配的认证失败后，可通过 **Repair credentials** 更新实际失败的受管本地连接，并明确重启其服务；远程连接显示服务端修复指引。[恢复步骤](../start/configuration.md#认证失败后的恢复)。 |
| 对话标题栏中的 **Messages**（窄窗口显示为 **Msg**） | 浏览当前 Agent 已加载对话中的用户、助手和工具条目。按 Enter 查看该条目可用的操作；按 Tab 或 ←/→ 选择操作，按 Enter 执行，按 Esc 返回条目列表且保留草稿。Full output 会打开已保存结果，或在没有保存结果时打开完整的内联结果。脚本有多份输出时，可继续选择要查看的单个动作。只读的回退历史组不是消息操作目标。 |
| Ctrl+O | 一起展开或收起可见的对话预览，包括工具组和 Agent 卡片。 |
| 在工具输出查看器中按 S | 给选中的保留输出命名。在命名提示中输入 `NAME --replace` 可更新已有名称。 |
| 鼠标左键拖动 | 高亮选区，松开时将所选文字复制到剪贴板。 |
| 鼠标滚轮、Page Up、Page Down | 滚动对话。 |
| Ctrl+End，或阅读历史时按 Alt+↑ | 回到最新输出并恢复跟随。 |
| 编辑区的 ↑、↓ | 在多行草稿内移动，到边缘后滚动对话。Ctrl+↑ 和 Ctrl+↓ 浏览已发送的提示词并返回草稿；队列焦点下则在可编辑的排队输入间移动。 |
| 菜单中的方向键、Tab、Enter | 方向键移动高亮项，Tab 补全可编辑文本，Enter 执行选中的动作。 |
| `exit` 或 `quit` | 单行草稿只有这一个小写词、可带首尾空格且没有附件时，Enter 或 Alt+Enter 会在草稿旁打开 **Exit kuku?**。按 Y 退出，按 Esc 继续编辑。 |

在选择弹窗中点击一行可高亮该项。点击也可切换正在查看的 Agent 或 Session、打开模型 ID 输入框，或查看对话详情和保留输出。点击标题栏的 **Messages** 可打开条目选择器，再点击条目进入其操作列表，点击操作即可执行。应用模型档或思考设置、决定授权、撤回输入和重启仍须按 Enter 确认。

空闲且输入框为空时，连续按两次 Esc 选择要回退的用户消息。在当前最早的消息处按 ↑ 或 Page Up，可加载更早历史且保留所选消息；选择器显示加载或重试反馈。有界窗口满后会移除较新的未选中消息；重新打开回退可返回最近消息。Enter 会先读取该条消息的完整原文，再打开确认页；原文加载失败可按 Enter 重试，Esc 取消且不改变草稿或历史。确认和可选文件恢复的作用见[消息回退](../how-it-works/sessions.md#消息回退)。

Continue 回执提示只短暂显示，不会替换已有错误。窄屏的聚焦控件提示也会保留完整 Esc 动作。

### 斜杠命令

命令和别名都可以搜索，列表只使用主命令名。Tab 补全主命令并保留参数。选择条目或提交完整斜杠命令都会执行同一操作。每项操作首次直接使用别名时，输入框旁会提示主命令名。`/reload` 保留给配置重载。

| 命令 | 别名 | 行为 |
|---|---|---|
| `/new` | `/new-session` | 打开一份空白草稿，之前的 Session 仍可打开。 |
| `/resume` | `/sessions` | 查找工作区中已有的 Session。 |
| `/agents` | `/agent` | 搜索 Session 中的 Agent、切换接收者或新建 Agent。选择器显示活动、待决请求、队列数量和可用的任务摘录。 |
| `/jobs` | `/job` | 列出所选 Agent 的后台任务和状态。选中任务可取消或读取保留输出。用 `/jobs cancel <id>` 按 ID 取消。 |
| `/model` | `/models` | 选择、添加或编辑模型路由。Ctrl+L 打开同一个选择器。 |
| `/thinking` | `/think`、`/effort` | 显示继承的模型默认值和受支持的等级，勾号标记当前选择。Enter 将高亮项应用到当前 Agent。**Use default** 清除该 Agent 的覆盖设置。对于可写入的本地模型路由，在等级上按 D 后用 Enter 确认，即可保存为模型默认值并保留 Agent 的显式覆盖。 |
| `/statusline` | `/status-line` | 打开 **Status line settings**，预览、选择、排序并保存状态栏字段。 |
| `/save` | `/save-value` | 在发送前给粘贴文本或当前草稿命名。 |
| `/values` | `/value`、`/saved-values` | 浏览保存值，选择一项读取或附到草稿。 |
| `/server` | `/service` | 检查已连接的服务。按 Ctrl+R 重连。 |
| `/restart` | `/restart-service` | 确认重启受管本地服务。有工作运行时会延后重启。 |
| `/web` | `/browser` | 在浏览器中打开对话。 |
| `/quit` | `/exit`、`/q` | 离开终端客户端。已接纳的工作在服务端继续。 |

### 阅读对话

每轮包含用户消息、工具执行和助手正文。同一轮内连续成功的只读检查默认归组，显示读取、搜索和列表等调用的数量。文件修改、运行中的调用、失败和授权请求仍单独显示。**List Agents** 汇总参与者及其活动，**Query Session** 汇总匹配的事件。使用标题栏中的 **Messages** 可检查单次调用并操作所选条目。

翻阅较早的输出时，点击输入区旁的 **Jump to latest** 可恢复跟随。

轮次统计在回答末尾显示一次，随正文滚动。浏览历史时，当前轮次和返回入口浮在输入框上方，共用输入框的左边线和文字内边距。按可用宽度显示耗时、token、缓存命中率与工具次数。

命令卡顶部用操作的简短说明作标题，缺少说明时显示 **Run command**。卡片预览可执行程序及参数，或所选 shell 与命令文本。底边显示观察到的执行状态和已知退出码。识别出测试报告时会显示程序报告的统计。点开卡片可查看记录的参数、捕获详情和可用证据。已完成的写入和编辑使用代码块样式。写入预览以实际记录的内容为准，源码按语言高亮，Markdown 使用正文渲染。编辑显示实际增删的行，卡片底边显示写入或变更的行数。

工作区脚本卡显示已完成的宿主动作和观察到的文件变更。展开后可查看各动作结果，包括部分完成和结果未知的情况。在 **Messages** 中选择脚本，再选 **Full output**，即可查看单个脚本动作的保留捕获。

发送的文字会立即出现在输入框上方的 **Pending inputs**。同一条输入会显示发送、回执、排队或失败状态，接纳后进入对话。重新打开时可找回未确认和排队的输入。草稿为空时按 ↑，或点击可见的普通排队输入，即可选中该条输入；选中状态绑定到其命令 ID，列表变化时仍会定位到同一条输入。Enter 打开独立编辑器，Delete 撤回。回执先于完整详情到达时，界面保留这次操作，直到同一条输入的快照到达；目标、焦点或草稿改变时取消等待。Running 或清理时 Esc 打断工作，其他状态返回输入框。如果该输入已离开队列，界面会提示选择已过期，不会操作其他行。`/queue`、`/queued`、`/withdraw` 和 `/reply` 不在操作选择器中。待决问题或权限请求会在输入框上方自动显示且不会抢焦点；按 F2 或点击卡片可聚焦。问题回复绑定到当前请求 ID；权限请求必须明确选择 Deny 或 Allow once 后按 Enter。Esc 离开卡片不会取消或授权，输入草稿时 y/n 也不会自动回复。如果 Agent 先接纳了输入，实际接纳的文字会保留在对话中。

其他 Agent 投递的消息会显示发送者地址，例如 `From main`。开启一轮的消息也会显示这个地址。

Markdown 边输出边排版，尚未闭合的代码围栏也可阅读。接入正在生成的回复时，终端可以显示已流出文本的有界前缀。已提交的回复会替换临时预览。

Markdown 回复支持任务列表、嵌套引用和列表，以及行内的 `<sup>`、`<sub>` 和 `<br>`。列表折行后，续行与该项正文对齐。单击带下划线的 HTTP 或 HTTPS 链接可在默认浏览器打开其目标地址。拖过链接可以选中文字。代码和源码视图保留原文。

打断正在流式输出的回复时，有界预览会保留在原轮次中，重新打开 Session 后也能看到。

助手回复中标为 `md` 或 `markdown` 的围栏默认预览其中的文档，包括标题、列表、表格和代码块。标签不区分大小写。内部代码按标注的语言显示。普通代码围栏按原样显示内容，支持的语言带语法高亮。

用户消息、命令、Agent、文件预览和代码块都在正文四周保留内边距，窄屏时缩小左右留白。Agent 卡片和代码块占满各自缩进层级的可用正文宽度，独立的 Agent 卡片有自己的边框，与相邻命令之间留一行空白。展开时 Details 入口保留在任务和回复详情上方。卡片顶部显示 Agent 名称。点已确认的 Agent 卡片右上角 **Open agent →** 可直接切换到它的会话视图，窄屏显示 **Open →**。创建指令在独立标签下换行，默认预览最多三行，较长指令可点 Details 阅读。任务和回复各用单行预览。同一 Agent 的创建和任务投递共用一张委派卡片，展开可检查各次操作。只涉及委派双方的状态查询收入该卡片的时间线；涉及其他 Agent 的查询另行汇总。已完成回复的思考阅读入口紧贴助手标题。

模型思考时，助手回复的第一行会显示轻微动效；有公开思考内容时，这里按终端自己的节奏逐字显示一行摘录。回答从同一行接上。点击 **View thinking** 或选择条目的 **Thinking** 操作，可以在消息原来的位置阅读已收到的思考内容。阅读区最多显示五行，回答继续显示在下方。方向键或阅读区上的滚轮可翻阅思考，Esc 或再次点击标题可收起。复制和 Markdown 源码视图使用所选回复正文。工具调用前的思考可在 **Messages** 中查看。

### 状态与用量

空白会话显示 kuku 标题。空间充足时，会话名和操作提示在 kuku 名称下左对齐。开始工作后，会话标题缩为一行。默认状态栏最左侧显示生效模型和解析得到的思考等级，例如 `gemini-4-neon · High`。`Default` 表示由模型服务选择等级，实际值未知。待生效的设置显示在 **Next model request** 中。底栏还显示连接状态、所选 Agent 和工作区。手机宽度下，工作区、上下文用量和平均 TPS 移到第二行。窗口很矮时底栏收为一行，为草稿留出空间。点击草稿文字可将光标移到对应字的位置，自动折行后也可直接点击。耗时紧跟当前活动名称。

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

`--json` 输出最终 `type: "session"` 对象，含 `event`、`session_id`、`agent`、`status`、`turns`、`response`、`error`、`config`、`usage`、`permission`、`question` 和 `duration_ms`。`--stream-json` 另含瞬时进度和持久回执信息。子 Agent 查询因输出上限重试时，`--stream-json` 会输出 `type: "model_recovery"` 行，包含 conversation、turn、失败／重试请求 ID、原因和尝试次数。慢观察者可能丢失瞬时增量。最终响应来自已提交状态。Provider 用量及未知值遵循 [Session SDK](../how-it-works/sessions.md) 约定。

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
| models | `kuku config models` |
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
