# Tools

本页描述 CLI 和 server 使用的公开 Session 默认工具面。Preset 或显式工具选择可以缩小范围。

## Built-In Tools

| Tool | Required args | Optional args | Risk |
|---|---|---|---|
| `find_files` | none | `path`, `pattern`, `max_depth` | `read` |
| `read_file` | `path` | `offset`, `limit` | `read` |
| `view_image` | `path` | 无 | `read` |
| `stat_file` | `path` | `version` | `read` |
| `read_output` | `id` 或 `name` | `offset`, `limit`, `search` | `read` |
| `search_text` | `pattern` | `path`, `include`, `view`, `offset`, `limit`, `context` | `read` |
| `fetch_url` | `url` | none | `read` |
| `query_session` | none | `search`, `kind`, `conversation`, `after`, `from_turn`, `to_turn`, `limit`, `skip_rolled_back` | `read` |
| `edit_file` | `path`, `old_text`, `new_text` | `replace_all`, `expected_version`, `brief` | `edit` |
| `write_file` | `path`, `content` | `expected_version`, `brief` | `edit` |
| `remove_file` | `path` | `expected_version`, `brief` | `edit` |
| `run_command` | `program` | `args`、`cwd`、`background`、`title` | `command` |
| `run_js` | `code` | `background`、`title` | `execute` |
| `list_jobs` | 无 | `before` | `session` |
| `wait_job` | `job_id` | 无 | `session` |
| `cancel_job` | `job_id` | 无 | `session` |
| `save_value` | `name`，再选 `output_id` 或 `value` | `replace` | `session` |
| `list_values` | 无 | `prefix`、`after`、`limit` | `read` |
| `remember_memory` | `scope`, `kind`, `text` | none | `edit` |
| `forget_memory` | `scope`, `text` | none | `edit` |

工具指引按 Agent 实际可用的能力组装。只回复的 Agent 不加载工具流程。委派和记忆写入指引只发给具备相应工具的 Agent。编辑和写入中的 `brief` 是可选的显示标签。

Session 在内置 DeepSeek Responses 路由上为声明支持图像输入的模型提供 `view_image`。它读取工作区中最大 1 MiB 的 PNG 或 JPEG，沿用文件读取的路径授权。模型请求按时间从近到远附带最多四张不同图像，以剩余请求字节预算为限。省略图像时，工具结果仍显示对应快照。 [DeepSeek Responses 图像输入](https://api-docs.deepseek.com/zh-cn/guides/responses_api/#image-input)

条件 Skill 工具：

- `list_skills(offset?, limit?)`
- `search_skills(query, offset?, limit?)`
- `use_skill(skill_name)`

启用默认 skill Tool surface 时，运行时会一起暴露 `list_skills`、`search_skills` 和 `use_skill`。

## Session 协作工具

| Tool | Required args | Optional args | Risk |
|---|---|---|---|
| `ask_user` | `question` | none | `session` |
| `create_agent` | `address` | `instructions`, `tools`, `preset`, `tier`, `workspace` | `session` |
| `send_message` | `to`, `message` | `wake`, `values` | `session` |
| `list_agents` | none | `wait_for` | `session` |

`create_agent` 创建稳定、扁平的 Session 内地址及独立上下文，可选的 `preset` 提供联系卡。应传入接收方所需的完整指令和任务材料。`send_message` 向已有 Agent 投递，返回持久投递回执，不代表最终工作结果。`wake` 默认为 true；false 仅入箱，等待显式推进。`list_agents` 默认读取精简状态列表。传入目标地址 `wait_for` 可等待该 Agent 当前执行结束并读取已完成的回复，最多等待 30 秒，超时返回当前状态。已结束的回复可以附带 `last_request`，其中包含请求所用的模型、请求指定的服务档位和供应商回报的服务档位。缺少依据的字段省略。

模型创建 Agent 时指定地址，并按需选择角色指令、工具白名单、preset、tier 和工作目录。宿主通过 SDK 配置供应商参数。

`ask_user` 持久化问题，并在当前工具批次边界让出执行。宿主通过 SDK、CLI `reply` 或 server 回复端点，使用精确请求标识作答。问题和授权的等待语义见 [Session](../how-it-works/sessions.md#等待与回复)。

`--no-agents`（或 `AgentConfig.no_agents`）移除 `create_agent`、`send_message` 和 `list_agents`，保留 `ask_user`。宿主参数见 [CLI](cli.md#kuku-run)。

## Conversation and Ledger Inspection

`query_session` 用于读取当前可见 conversation 上下文之外的历史 Session 事件。

重要过滤项：

- `conversation`：限制到某个 conversation address
- `kind`：限制到某个事件类型
- `after`：只返回 id 大于该值的事件
- `from_turn` 和 `to_turn`：相对 turn 窗口
- `skip_rolled_back`：默认是 `true`

`query_session` 用于历史回忆，不应用来重复读取当前消息中已经存在的数据。

## 运行本机程序

把可执行程序传入 `program`，在 `args` 中把每个参数写成一个字面字符串。程序无需参数时可省略 `args`。参数可以是空字符串，也可以包含空格、引号或 shell 元字符。工作目录默认为 Agent 的工作区。可用 `cwd` 指定相对工作区的目录或绝对目录。各个平台都用 `./` 指定相对 `cwd` 的可执行文件路径。程序从标准输入立即读到 EOF。

```json
{"program":"git","args":["status","--short"]}
```

需要执行 shell 脚本时，显式传入解释器和参数。例如，在 Windows 上可以用 `{"program":"cmd.exe","args":["/D","/C","build.cmd"]}` 运行批处理文件。

```json
{"program":"sh","args":["./verify.sh"]}
```

[嵌入式宿主](sdk.md#工作区执行)可以设置进程截止时间。命令和脚本共用 [Session Job](#session-job) 行为。顶层调用可用 `title` 说明操作目的，卡片和运行中的 Job 会显示这段说明。

进程正常退出后，工具状态可以为 `ok`，同时 `exit_code` 为非零值。Shell 调用报告外层 Shell 的退出码。串接命令或使用管道时，保留每项必要检查的失败状态。命令卡片显示用时，并单独标记 stderr。检查结构化事实中的 `termination`、`exit_code` 和 `capture_complete`。

Agent 有 `read_output` 时，短命令输出直接显示。超过 2 KiB 或 40 行时返回有界视图和保留输出引用。视图展示识别到的构建／测试证据，或描述各条流已观察到的开头，对多行文本附上有界示例。对 JSON Lines，可以提示观察到的字段以及如何用可用的脚本处理。用 `read_output` 查看相关范围。TUI 可以独立显示输出片段。

原生调用 Cargo 构建、检查、Clippy、测试或执行常规 Pytest 时，识别到的长文本可以折叠进度和通过的测试，同时展示诊断和失败断言。摘要会标明规则，并给出一个可重新读取的字节范围。结构化的 `output_summary` 记录来源的流与合并后的左闭右开字节范围。可用 `read_output` 查看原始捕获。显式选择机器可读格式、仅收集测试或捕获不完整时，使用通用预览。

较长的 TypeScript 编译器纯文本报告可以按完全相同的错误代码和消息分组。每组包含次数和示例源码位置。彩色格式和多行诊断使用通用预览。

较长的 `go test` 纯文本报告可以统计各包的结果，并保留失败断言、编译诊断和对应的原文范围。JSON、详细模式和无法识别的报告使用通用预览。

较长的 `node --test` spec 报告可以汇总测试计数，并保留失败断言、TODO 诊断和对应的原文范围。其他报告器和无法识别的报告使用通用预览。

较长的 Git 短状态和按行输出的 porcelain v1 可以将记录按暂存、未暂存、冲突、未跟踪和忽略状态分组。视图保留数量、带 Git 引号的示例行，以及如何在 `run_js` 中筛选原始状态行的提示。其他 Git 状态格式使用通用预览。[Git 状态格式](https://git-scm.com/docs/git-status#_output)

Session 的默认预览上限为 8 KiB，包含摘要和保留输出引用。即使捕获完整，预览也可能截断。长命令失败且捕获完整时，通用预览包含 stderr 末尾。stderr 为空时，显示 stdout 末尾。片段可以同时附带识别到的测试统计和构建或测试摘要。程序缺失、启动失败、超时和取消有不同的终止原因。识别出的 Rust 测试和 pytest 摘要会提供程序报告的统计与有界证据。[SDK 执行事实](sdk.md#工作区执行)

## Session Job

顶层 `run_command` 或 `run_js` 在观察窗口内结束时直接返回结果。前台窗口默认为 10 秒，设置 `background: true` 时改为 1 秒。窗口结束时若仍在运行，工具返回 Job 句柄，其中 `structured.kind` 为 `"job"`，`structured.job_id` 是 Job 标识。模型结果和结构化事实均包含 `on_session_close: "cancel"`。Agent 可以继续独立工作。

[Job 生命周期与结果投递](../how-it-works/sessions.md#后台-job) · [权限](../how-it-works/permissions.md)

- `list_jobs({})` 按从新到旧返回当前 Agent 的最多 25 条 Job 摘要。将返回的 `next_before` 作为 `before` 传入可继续翻页。
- `wait_job({job_id})` 等待当前 Agent 的指定 Job 结束。下一步依赖这一结果时使用。
- `cancel_job({job_id})` 请求取消当前 Agent 的指定 Job，并等待清理。

使用命令的保留输出引用可检查其捕获内容。[嵌入宿主的 Job API](sdk.md#工作区执行)

## JavaScript 组合工具

`run_js({code})` 在一次性的嵌入式 QuickJS worker 中运行 JavaScript 异步函数体，支持顶层 `await`。给外层工具调用传入 `background: true` 可启动 [Session Job](#session-job)。可以使用标准 JavaScript，包括 `JSON`、正则表达式及用于独立操作的 `Promise.all`。后一动作的参数能由前一结果直接计算时，使用 `await` 在脚本内连续执行。下一步需要模型判断时，返回简短的观察结果。`tools` 提供该 Agent 可在脚本中调用的方法。`tools.methods` 列出宿主方法和参数 schema。

语法错误有定位信息时，行号和列号对应提交的 `code`。

大多数宿主方法返回 `{status, result, truncated}`。确认 `status: "ok"` 后，从 `result` 读取结构化数据。动作失败时会附带 `error`。将列表或分页当作完整结果之前，检查 `truncated`。例如，`tools.read_file({path})` 提供原始 `result.raw_text`、SHA-256 `content_hash`，以及从 `start_byte` 到 `end_byte` 的左闭右开字节范围。脚本可用已观察的范围继续编辑，无需把文件内容返回给模型。同一文件版本的读取覆盖全文后，还可以覆盖或移除该文件。在同一次调用中，脚本也可以编辑或覆盖由自身写入完整内容且已确认成功的确切版本。动作仍需经过宿主授权与版本检查。

脚本可以返回较小的 JSON 值，为模型提供所需的证据。脚本结束时若没有返回值，`console.log(...values)` 可交付有界文本。显式返回值优先，包括 `null`。处理大输出时，用 `tools.output_lines({id, stream: "stdout"})` 逐行遍历，只保留计算所需的状态。原始字节可以通过[保留输出](#保留的工具输出)读取。JavaScript 变量只在当次调用中存在。可以把选中的结果存为[命名值](#命名会话值)，也可以按用户要求把交付物写入工作区。

`await tools.run_command({program, args?, cwd?})` 等待命令结束。要让脚本在后台运行，在外层 `run_js` 调用上设置 `background: true`。进程退出时，只要有整数退出码，就返回 `{exit_code, duration_ms, capture_complete, output_ref, stdout, stderr}`，包括非零退出码。拒绝派发、启动失败、超时或取消等情况会抛出错误。错误提供 `status`，并在可用时附带 `termination`、`signal` 或 `output_ref`。

两个流的方法相同。`await run.stdout.text()` 读取完整的解码文本，上限为 1 MiB，`await run.stdout.json()` 将同一文本解析为 JSON。用 `for await (const line of run.stderr.lines())` 逐行读取去掉 LF 或 CRLF 的解码文本，单行上限为 128 KiB。遇到捕获不完整或有损解码时，这些方法会报错。需要查看已保留的部分输出或原始字节时，使用 `tools.output_chunks({id: run.output_ref.id})`。保留输出的分页读取经过 `read_output` 授权。

以下示例检查 Git 工作树，从完整的状态输出中统计条目数。

```javascript
const run = await tools.run_command({program: "git", args: ["status", "--porcelain"]});
const {exit_code, duration_ms, capture_complete, output_ref} = run;
if (exit_code !== 0 || !capture_complete) {
  return {exit_code, duration_ms, capture_complete, output_ref};
}
const entries = (await run.stdout.text()).split("\n").filter(Boolean).length;
return {exit_code, duration_ms, entries, output_ref};
```

`output_chunks({id, offset?, limit?, search?})` 逐页读取有界输出页，其中包含原始流片段和字节。默认扫描窗口为 65,536 字节，每次请求最多 80,000 字节。`output_lines({id, stream?, offset?, limit?})` 从未过滤的输出页逐行读取去掉末尾换行符的解码文本。两个迭代器都可用 `name` 代替 `id`，解析名称后继续使用同一输出 ID。每行携带流标识、字节偏移和完整性标记。脚本宿主方法独立于脚本返回值报告拒绝、失败和取消。

`await tools.search_output({id, text, stream?, offset?, limit?})` 在保留输出中匹配 UTF-8 原始字节。可以用 `name` 代替 `id`，并沿用 `read_output` 授权。每次请求在 Rust 中扫描最多 8 MiB 的新保留字节，默认最多返回 32 条匹配，上限为 128 条。先检查 `status` 再使用 `result`。只要 `result.has_more` 为真，就传入 `result.output_id` 和 `result.next_offset` 续查，空页也一样。

匹配按结束位置排列。每条匹配包含流标识 `stream`、起始偏移 `offset` 和不包含末尾的 `end_offset`，偏移采用合流字节坐标。同一流的匹配可以跨帧，回读交错范围时应按 `stream` 筛选。`capture_complete` 描述原始捕获是否完整，扫描是否还有下一页由 `has_more` 表示。

保存大型派生结果时，先从 `await tools.create_output({})` 取得 `result.id`，再用 `await tools.append_output({id, text})` 分批追加 UTF-8 文本。检查各操作的状态。调用 `await tools.finish_output({id})` 后，检查 `result.capture_complete` 再使用新的 `result.output_ref`。原始捕获仍可单独读取。

输出草稿只在当前脚本调用内有效。见[草稿动作的授权规则](../how-it-works/permissions.md)。

## 保留的工具输出

`read_output` 按输出 ID 或当前 Agent 的准确值名读取 Session 保留数据。使用运行中命令的输出 ID 时，返回读取时已经写完的捕获帧。每次调用都须经过宿主授权。下表是直接工具的限额，脚本读取器的参数定义见 `tools.methods`。

| Argument | Contract |
|---|---|
| `id` | 由 1–128 个 ASCII 字母、数字、`-` 或 `_` 组成的输出 ID。`id` 和 `name` 二选一。 |
| `name` | Agent 持有的保存值的准确名称 |
| `offset` | 按捕获顺序跨 stdout 和 stderr 的非负原始字节偏移；默认 `0` |
| `limit` | 扫描的原始字节数预算，必须为正整数。默认 `4096`，最多扫描 `8192` 字节。 |
| `search` | 可选的字面量过滤：`1`–`4096` 个 UTF-8 字节，且不得长于 `limit` |

`model_content` 返回完整 JSON，最多 16,384 个 UTF-8 字节；`structured` 包含相同的值。编码开销或无效 UTF-8 的替换可能使返回页小于请求的扫描上限。工具保留读取器的原始字节偏移，不切割 JSON 文本。如果搜索在最小扫描窗口下仍无法满足结果上限，会返回错误并要求缩短字面量。

`segments` 保留流标识、捕获顺序、原始偏移和字节数。通过 `next_offset` 和 `has_more` 续读；下一次读取应传入 `next_offset`，不要自行加上 `limit`。字面量搜索的相邻扫描窗口可能保留重叠。匹配页为空但 `has_more: true` 时仍需继续。`filtered` 表示搜索结果，`lossy_utf8` 表示替换了无效或被窗口切开的 UTF-8 字节。

文本视图移除 ANSI 颜色码。Windows 优先按 UTF-8 读取；对于标准输出和标准错误中的完整文本行，也会尝试宿主的 OEM 代码页。二进制数据和不完整字符仍可能显示替换符。显示文本与捕获字节不同时，`raw_bytes` 保留原始字节。其他情况下，可由 UTF-8 文本直接得到原始字节。SDK 调用方可统一使用 `OutputSegment::as_bytes()`。

`complete` 表示写入器已完成收尾。当 `has_more` 和 `complete` 都为假时，读取已到达当前可用输出的末尾。检查后续输出时，继续使用 `next_offset`。`capture_truncated` 表示捕获配额导致输出丢弃。`cleanup_incomplete` 表示管道清理达到截止时间。`summary` 包含读取器选出的构建或测试结果行。捕获数据的生命周期和清理语义见 [输出与清理](../how-it-works/sessions.md#输出与清理)。

每个调用方自行提供偏移。重复读取已完成的输出不会消费数据，UI 读取也不会推进模型游标。无效 ID、参数类型、扫描上限或搜索长度会返回错误，不进行越界读取。

## 命名会话值

用 `save_value({name, output_id})` 给已有捕获命名，无需复制其字节，也可用 `save_value({name, value})` 保存最多 16 KiB 的字面 JSON 值。字符串按 UTF-8 文本保存。名称区分大小写，以 ASCII 字母开头，最多 64 个 ASCII 字母、数字、下划线或连字符。

用 `list_values({prefix?, after?, limit?})` 查询名称、类型及引用。结果包含 `values` 和 `next_after`，每页按名称顺序返回最多 32 项。将 `next_after` 作为 `after` 传入可继续翻页。用 `read_output({name})` 按需取内容。名称绑定归当前 Agent 所有，关闭并重开 Session 后仍可使用。重复保存同一来源是幂等操作。用 `replace: true` 把现有名称绑定到新的来源。

脚本可用这些工具时，通过 `await tools.save_value({name, value})` 保存计算结果，或传入 `output_id` 绑定保留输出。用 `await tools.list_values()` 查询当前 Agent 的名称。两种方法沿用对应直接工具的参数和授权。

用 `send_message({to, message, values: ["build_log"]})` 在下达任务时向接收方分享固定的值引用。每条消息最多可分享八个准确名称或完整输出引用。以后替换 `build_log` 的绑定，已投递的引用仍然有效。接收方可以用共享的 ID 读取该引用。

## Tool Result Envelope

所有 Tool 都返回统一的顶层结构：

| Field | Meaning |
|---|---|
| `status` | `ok`、`error`、`blocked` 或 `cancelled` |
| `summary` | 简短结果摘要 |
| `model_content` | 供下一步使用的证据 |
| `truncated` | 证据是否截断，包括命令捕获不完整的情况 |
| `structured` | 可选的机器可读细节 |

文件变更中与快照有关的错误会保留 `structured.kind: "error"`，并增加用于恢复的 `reason_code`，例如 `snapshot_required`、`full_snapshot_required`、`snapshot_stale` 或 `old_text_not_visible`。

在单个推进中的 Agent 内，kuku 会让 `read_file`、`stat_file`、`edit_file`、`write_file`、`remove_file`、`remember_memory`、`forget_memory`、`run_command` 和 `run_js` 槽位彼此串行。其他只读工具仍可并发。同一 Session 中不同 Agent 可并发推进。快照哈希检查能发现读写之间的变化，包括外部进程造成的变更。

## Notes By Tool

- `find_files` 默认列出直接子项，给出 `pattern` glob 时递归搜索。`max_depth` 可覆盖默认值，`0` 列出直接子项，`1` 再深入一层目录。相对目录路径以 `/` 结尾，扫描跳过常见构建目录。长列表会说明总数和如何缩小查询。
- `read_file` 返回带行号内容，并支持分页。
- `stat_file` 返回路径类型（`file`、`directory`、`other` 或 `missing`）、是否存在及可用的文件大小。设置 `version: true` 时会流式读取普通文件的字节并计算 SHA-256 `content_hash`，结果只提供哈希，不包含文件正文。
- `search_text` 基于正则，支持 `files`、`lines` 和 `count` 视图。结构化 `matches` 提供路径和行号，`lines` 还提供有界文本。通过 `next_offset` 续读。可将匹配的原文用于编辑，也可读取相邻行以补充上下文。
- `fetch_url` 下载最大 50 MiB 的 HTTP(S) 响应正文。即使服务器返回错误响应，结果也会包含 HTTP 状态码。在 Session 内，完整正文必须能存入可用的输出配额。下载完成后，发起请求的 Agent 可将 `output_ref.id` 传给 `read_output` 读取正文。取消下载会丢弃已接收的部分。独立查询返回临时文件路径。工具会拒绝带内嵌密码的 URL。
- `query_session` 会过滤 Session 账本，默认排除 rolled-back 事件，并截断单条事件内容。
- `edit_file` 可直接替换已知的 `old_text`，只要它在当前文件中仅出现一次。文件其他部分的修改会保留。若原文出现多次，工具可接受在当前版本已观察范围内的唯一匹配。多个位置都符合时，提供更多相邻原文来定位。`replace_all=true` 需要掌握该版本的整文件内容。只有在匹配 CRLF 文件确有需要时，LF 输入才会映射为 CRLF，并保持替换文本的换行风格。写入时文件哈希仍须一致。
- `write_file` 创建文件时会一并创建工作区内缺失的父目录。它可以覆盖 Agent 在有效历史中完整读取过的文件版本，或由它自身写入完整内容且已确认成功的版本。当前路径和哈希都必须与证据一致。局部编辑不足以确认变更后的整文件内容。
- `remove_file` 在 `expected_version` 与当前哈希一致时移除普通文件。省略该参数时，需要同一版本的整文件读取。移除二进制文件可用 `stat_file({path, version: true})` 获取哈希，再作为 `expected_version` 传入。末级路径为符号链接时会拒绝该请求。
- 文件工具的 `expected_version` 可使用返回的 `sha256:` 版本串，也可使用同一完整的 64 位十六进制 SHA-256 摘要。内容检查比较全部摘要位。
- `edit_file` 和 `write_file` 可传入先前读取或已确认写入的 `content_hash` 作为 `expected_version`，进行附加版本检查。如果观察之后文件任意位置发生变化都应拒绝操作，就传入该版本。
- `remember_memory` 和 `forget_memory` 通过专用 API 写入 memory 文件。

## Memory Tool Enums

对 `remember_memory`：

- `scope`: `global` 或 `project`
- `kind`: `how_to_work`、`what_is_true` 或 `where_to_look`

对 `forget_memory`：

- `scope`: `global` 或 `project`
