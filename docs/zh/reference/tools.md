# Tools

## Built-In Tools

| Tool | Required args | Optional args | Risk |
|---|---|---|---|
| `find_files` | none | `path`, `pattern`, `max_depth` | `read` |
| `read_file` | `path` | `offset`, `limit` | `read` |
| `search_text` | `pattern` | `path`, `include`, `view`, `offset`, `limit`, `context` | `read` |
| `fetch_url` | `url` | none | `read` |
| `fetch_web` | `url`, `prompt`, `model_tier` | none | `read` |
| `query_session` | none | `search`, `kind`, `conversation`, `after`, `from_turn`, `to_turn`, `limit`, `skip_rolled_back` | `read` |
| `edit_file` | `path`, `old_text`, `new_text`, `brief` | `replace_all` | `edit` |
| `write_file` | `path`, `content`, `brief` | none | `edit` |
| `run_command` | `command`, `timeout`, `brief` | none | `command` |
| `remember_memory` | `scope`, `kind`, `text` | none | `edit` |
| `forget_memory` | `scope`, `text` | none | `edit` |

条件工具：

- `agent(to, message, tier?)`
- `list_skills(offset?, limit?)`
- `search_skills(query, offset?, limit?)`
- `use_skill(skill_name)`

启用默认 skill Tool surface 时，运行时会一起暴露 `list_skills`、`search_skills` 和 `use_skill`。

## `agent(to, message, tier?)`

`agent` 会把工作委派给某个命名 agent contact card，并在单独的 conversation 中执行。

参数：

| Arg | Required | Meaning |
|---|---|---|
| `to` | yes | conversation address，例如 `review` 或 `review/api` |
| `message` | yes | 发送到该 conversation 的任务文本 |
| `tier` | no | 首次绑定新 conversation 时使用的模型 tier |

行为：

- `main` 是保留地址，会被拒绝。
- 未知根联系人会报 `unknown agent contact: <name>`。
- `to` 的根 segment 用于选择 agent contact card。
- 复用同一个 address 会继续同一 conversation。
- `tier` 只在首次绑定时生效。
- 如果继续已有 address 还传入 `tier`，运行时会拒绝。
- 如果继续已有 address，但绑定身份已经变化，运行时会拒绝。

当工作需要隔离上下文和独立 transcript 时使用 `agent`。不要把它当成“再跑一个进程”的同义词。

## Conversation and Ledger Inspection

`query_session` 用于读取当前可见 conversation 上下文之外的历史 Session 事件。

重要过滤项：

- `conversation`：限制到某个 conversation address
- `kind`：限制到某个事件类型
- `after`：只返回 id 大于该值的事件
- `from_turn` 和 `to_turn`：相对 turn 窗口
- `skip_rolled_back`：默认是 `true`

`query_session` 用于历史回忆，不应用来重复读取当前消息中已经存在的数据。

## Tool Result Envelope

所有 Tool 都返回统一的顶层结构：

| Field | Meaning |
|---|---|
| `status` | `ok`、`error`、`blocked` 或 `cancelled` |
| `summary` | 简短结果摘要 |
| `model_content` | 供下一步使用的证据 |
| `truncated` | `model_content` 是否被截断 |
| `structured` | 可选的机器可读细节 |

## Notes By Tool

- `find_files` 返回相对路径，并跳过常见构建目录。
- `read_file` 返回带行号内容，并支持分页。
- `search_text` 基于正则，支持 `files`、`lines` 和 `count` 视图。
- `fetch_url` 下载到临时目录，拒绝非 HTTP(S) URL 和内嵌凭证，并限制 50 MB。
- `fetch_web` 用于 HTML 类内容，限制 10 MB 响应体，小页面直接返回，大页面按请求的 `model_tier` 生成摘要。
- `query_session` 会过滤 Session 账本，默认排除 rolled-back 事件，并截断单条事件内容。
- `edit_file` 需要唯一 `old_text` 匹配和先前的 read snapshot。
- `write_file` 只有在先前完整读取文件后才允许覆盖。
- `run_command` 要求 `timeout` 以秒为单位。
- `remember_memory` 和 `forget_memory` 通过专用 API 写入 memory 文件。

## Memory Tool Enums

对 `remember_memory`：

- `scope`: `global` 或 `project`
- `kind`: `how_to_work`、`what_is_true` 或 `where_to_look`

对 `forget_memory`：

- `scope`: `global` 或 `project`
