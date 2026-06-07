# Tools

## 内建 Tools

| Tool | Required args | Optional args | Risk |
|---|---|---|---|
| `find_files` | none | `path`, `pattern`, `max_depth` | `read` |
| `read_file` | `path` | `offset`, `limit` | `read` |
| `search_text` | `pattern` | `path`, `include`, `view`, `offset`, `limit`, `context` | `read` |
| `fetch_url` | `url` | none | `read` |
| `fetch_web` | `url`, `prompt`, `model_tier` | none | `read` |
| `query_session` | none | `search`, `type`, `from_turn`, `to_turn`, `limit`, `skip_rolled_back` | `read` |
| `edit_file` | `path`, `old_text`, `new_text`, `brief` | `replace_all` | `edit` |
| `write_file` | `path`, `content`, `brief` | none | `edit` |
| `run_command` | `command`, `timeout`, `brief` | none | `command` |
| `remember_memory` | `scope`, `kind`, `text` | none | `edit` |
| `forget_memory` | `scope`, `text` | none | `edit` |

条件性 Tool：

- `agent`，必填参数为 `name`、`prompt`
- `list_skills`，可选参数为 `offset`、`limit`
- `search_skills`，必填参数为 `query`，可选参数为 `offset`、`limit`
- `use_skill`，必填参数为 `skill_name`

启用默认 Skill Tool surface 时，运行时会一并暴露 `list_skills`、`search_skills` 和 `use_skill`。

## Tool 返回包络

每个 Tool 都返回相同的顶层结构：

| Field | Meaning |
|---|---|
| `status` | `ok`、`error`、`blocked` 或 `cancelled` |
| `summary` | 简短结果说明 |
| `model_content` | 用于下一步的证据 |
| `truncated` | `model_content` 是否被截断 |
| `structured` | 可选的机器可读细节 |

## 各 Tool 说明

- `find_files` 返回相对路径，并跳过常见构建目录。
- `read_file` 返回带行号的内容，并支持分页。
- `search_text` 基于正则表达式，并支持 `files`、`lines` 和 `count` 视图。
- `fetch_url` 会下载到临时目录，拒绝非 HTTP(S) URL 和带嵌入凭证的 URL，并强制执行 50 MB 上限。
- `fetch_web` 用于 HTML 类内容，强制执行 10 MB body 上限，小页面直接返回，较大页面会用请求的 `model_tier` 做摘要。结果会短时间缓存。
- `query_session` 用于查询当前可见对话上下文之外的历史 Session 事件。`skip_rolled_back` 默认为 `true`，单个事件内容会被截断，总输出也有上限。
- `edit_file` 需要唯一的 `old_text` 匹配，以及先前的读取快照。
- `write_file` 只会在已有先前整文件读取快照时覆盖文件。
- `run_command` 要求 `timeout` 以秒为单位。
- `remember_memory` 和 `forget_memory` 通过专用 API 写入 memory 文件。

## Memory Tool 枚举

对于 `remember_memory`：

- `scope`：`global` 或 `project`
- `kind`：`how_to_work`、`what_is_true` 或 `where_to_look`

对于 `forget_memory`：

- `scope`：`global` 或 `project`
