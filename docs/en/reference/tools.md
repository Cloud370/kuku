# Tools

## Built-In Tools

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

Conditional tools:

- `agent` with required args `name`, `prompt`
- `list_skills` with optional args `offset`, `limit`
- `search_skills` with required arg `query` and optional args `offset`, `limit`
- `use_skill` with required arg `skill_name`

When the default skill tool surface is enabled, the runtime exposes `list_skills`, `search_skills`, and `use_skill` together.

## Tool Result Envelope

Every tool returns the same top-level shape:

| Field | Meaning |
|---|---|
| `status` | `ok`, `error`, `blocked`, or `cancelled` |
| `summary` | Short outcome line |
| `model_content` | Evidence for the next step |
| `truncated` | Whether `model_content` was cut |
| `structured` | Optional machine-readable detail |

## Notes by Tool

- `find_files` returns relative paths and skips common build directories.
- `read_file` returns line-numbered content and supports pagination.
- `search_text` is regex-based and supports `files`, `lines`, and `count` views.
- `fetch_url` downloads to a temp directory, rejects non-HTTP(S) URLs and embedded credentials, and enforces a 50 MB limit.
- `fetch_web` is for HTML-like content, enforces a 10 MB body limit, returns small pages directly, and summarizes larger pages with the requested `model_tier`. Results are cached briefly.
- `query_session` is for historical session events that are no longer in the visible conversation context. `skip_rolled_back` defaults to `true`, individual event content is truncated, and total output is capped.
- `edit_file` requires a unique `old_text` match and a prior read snapshot.
- `write_file` overwrites only after a prior full-file read snapshot.
- `run_command` requires `timeout` in seconds.
- `remember_memory` and `forget_memory` write the memory files through dedicated APIs.

## Memory Tool Enums

For `remember_memory`:

- `scope`: `global` or `project`
- `kind`: `how_to_work`, `what_is_true`, or `where_to_look`

For `forget_memory`:

- `scope`: `global` or `project`
