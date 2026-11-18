# Tools

本页描述 CLI 和 server 使用的公开 Session 默认工具面。Preset 或显式工具选择可以缩小范围。

## Built-In Tools

| Tool | Required args | Optional args | Risk |
|---|---|---|---|
| `find_files` | none | `path`, `pattern`, `max_depth` | `read` |
| `read_file` | `path` | `offset`, `limit` | `read` |
| `read_output` | `id` | `offset`, `limit`, `search` | `read` |
| `search_text` | `pattern` | `path`, `include`, `view`, `offset`, `limit`, `context` | `read` |
| `fetch_url` | `url` | none | `read` |
| `query_session` | none | `search`, `kind`, `conversation`, `after`, `from_turn`, `to_turn`, `limit`, `skip_rolled_back` | `read` |
| `edit_file` | `path`, `old_text`, `new_text`, `brief` | `replace_all` | `edit` |
| `write_file` | `path`, `content`, `brief` | none | `edit` |
| `run_command` | `command`, `timeout`, `brief` | none | `command` |
| `remember_memory` | `scope`, `kind`, `text` | none | `edit` |
| `forget_memory` | `scope`, `text` | none | `edit` |

条件 Skill 工具：

- `list_skills(offset?, limit?)`
- `search_skills(query, offset?, limit?)`
- `use_skill(skill_name)`

启用默认 skill Tool surface 时，运行时会一起暴露 `list_skills`、`search_skills` 和 `use_skill`。

## Session 协作工具

| Tool | Required args | Optional args | Risk |
|---|---|---|---|
| `ask_user` | `question` | none | `session` |
| `create_agent` | `address` | `connection`, `tier`, `model`, `preset`, `instructions`, `reasoning_effort`, `service_tier`, `max_output_tokens`, `no_agents`, `no_skills`, `bootstrap_skill`, `tools`, `workspace` | `session` |
| `send_message` | `to`, `message` | `wake` | `session` |
| `list_agents` | none | none | `session` |

`create_agent` 创建稳定、扁平的 Session 内地址及独立上下文，可选的 `preset` 提供联系卡。应传入接收方所需的完整指令和任务材料。`send_message` 向已有 Agent 投递，返回持久投递回执，不代表最终工作结果。`wake` 默认为 true；false 仅入箱，等待显式推进。`list_agents` 读取保留状态和最后持久输出，不消费观察流。

省略模型路由字段时，使用 Session 的默认 tier 或所选 preset 的 tier。将 `tier` 设为已配置的 `[model.NAME]` 名称，可选用对应的路由和默认设置。只有知道确切的 `[provider.NAME]` 条目名称时才填写 `connection`。

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

## 保留的工具输出

`read_output` 读取由 Session 持有、由 `run_command` 等工具返回的输出 ID。它与 `Session::read_output` 共用同一个有界读取器，不接受文件系统路径。即使该工具只读，仍须经过宿主授权。

| Argument | Contract |
|---|---|
| `id` | 必需的输出 ID：1–128 个 ASCII 字母、数字、`-` 或 `_` |
| `offset` | 按捕获顺序跨 stdout 和 stderr 的非负原始字节偏移；默认 `0` |
| `limit` | 扫描的原始字节数上限：`1`–`8192`；默认 `4096` |
| `search` | 可选的字面量过滤：`1`–`4096` 个 UTF-8 字节，且不得长于 `limit` |

`model_content` 返回完整 JSON，最多 16,384 个 UTF-8 字节；`structured` 包含相同的值。编码开销或无效 UTF-8 的替换可能使返回页小于请求的扫描上限。工具保留读取器的原始字节偏移，不切割 JSON 文本。如果搜索在最小扫描窗口下仍无法满足结果上限，会返回错误并要求缩短字面量。

`segments` 保留流标识、捕获顺序、原始偏移和字节数。通过 `next_offset` 和 `has_more` 续读；下一次读取应传入 `next_offset`，不要自行加上 `limit`。字面量搜索的相邻扫描窗口可能保留重叠。匹配页为空但 `has_more: true` 时仍需继续。`filtered` 表示搜索结果，`lossy_utf8` 表示替换了无效或被窗口切开的 UTF-8 字节。

当数据段的 `text` 替换了无效或被切开的 UTF-8 时，`raw_bytes` 保留原始字节。其他情况下，可由 UTF-8 文本直接得到原始字节。SDK 调用方可统一使用 `OutputSegment::as_bytes()`。

`complete` 表示写入器已完成收尾。`capture_truncated` 表示捕获配额导致输出丢弃；`cleanup_incomplete` 表示管道清理达到截止时间。即使返回的 JSON 页满足大小上限，这些标记也会明确保留。`summary` 包含读取器选出的构建或测试结果行。捕获数据的生命周期和清理语义见 [输出与清理](../how-it-works/sessions.md#输出与清理)。

每个调用方自行提供偏移。重复读取已完成的输出不会消费数据，UI 读取也不会推进模型游标。无效 ID、参数类型、扫描上限或搜索长度会返回错误，不进行越界读取。

## Tool Result Envelope

所有 Tool 都返回统一的顶层结构：

| Field | Meaning |
|---|---|
| `status` | `ok`、`error`、`blocked` 或 `cancelled` |
| `summary` | 简短结果摘要 |
| `model_content` | 供下一步使用的证据 |
| `truncated` | `model_content` 是否被截断 |
| `structured` | 可选的机器可读细节 |

与 snapshot 有关的 `edit_file` 和 `write_file` 错误会保留 `structured.kind: "error"`，并增加一个用于恢复的 `reason_code`：`snapshot_required`、`full_snapshot_required`、`snapshot_stale` 或 `old_text_not_visible`。

在单个推进中的 Agent 内，Kuku 会让 `read_file`、`edit_file`、`write_file`、`remember_memory`、`forget_memory` 和 `run_command` 槽位彼此串行。命令的写入路径无法静态判定，因此相互独立的命令也可能等待；其他只读工具仍可并发。同一 Session 中不同 Agent 可并发推进。该顺序只覆盖 Kuku 管理的槽位，不覆盖外部进程，因此 snapshot hash 检查仍然适用。

## Notes By Tool

- `find_files` 返回相对路径，并跳过常见构建目录。
- `read_file` 返回带行号内容，并支持分页。
- `search_text` 基于正则，支持 `files`、`lines` 和 `count` 视图。
- `fetch_url` 下载到临时目录，拒绝非 HTTP(S) URL 和内嵌凭证，并限制 50 MB。
- `query_session` 会过滤 Session 账本，默认排除 rolled-back 事件，并截断单条事件内容。
- `edit_file` 要求 `old_text` 唯一匹配一个仍在当前 conversation 有效历史中可见的 read snapshot。局部或截断读取只能授权实际展示的原始文件文本，而 `replace_all=true` 需要整文件 snapshot。只有在匹配 CRLF 文件确有需要时，LF 输入才会映射为 CRLF，并保持替换文本的换行风格。写入时文件 hash 仍须一致。
- `write_file` 只有在当前 conversation 的有效历史中仍可见先前的整文件 read snapshot 时，才允许覆盖已有文件。
- `run_command` 要求 `timeout` 以秒为单位。在 Windows 上，后台 shell 运行时不弹出控制台窗口，标准输出和错误输出仍保存在工具结果中。
- `remember_memory` 和 `forget_memory` 通过专用 API 写入 memory 文件。

## Memory Tool Enums

对 `remember_memory`：

- `scope`: `global` 或 `project`
- `kind`: `how_to_work`、`what_is_true` 或 `where_to_look`

对 `forget_memory`：

- `scope`: `global` 或 `project`
