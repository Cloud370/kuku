# CLI

## 入口模式

- `kuku` 在当前 workspace 中启动交互模式。
- `kuku run ...` 启动一次非交互运行。

## 顶层命令

| Command | Purpose |
|---|---|
| `kuku run <prompt...>` | 执行一个任务 |
| `kuku show <session-id>` | 显示某个 Session 的最终输出 |
| `kuku events <session-id>` | 显示已持久化的事件 |
| `kuku list` | 列出 Session |
| `kuku delete <session-id>` | 删除一个 Session |
| `kuku config ...` | 显示、校验或修改配置 |
| `kuku init` | 初始化配置和运行时目录 |
| `kuku prompts ...` | 显示或导出 Prompt 资源 |
| `kuku agents ...` | 列出或查看 Agent |
| `kuku skills ...` | 列出或查看 Skill |
| `kuku server ...` | 启动 HTTP API server |
| `kuku web ...` | 启动带内嵌 Web UI 的 HTTP server |

## `kuku run`

```text
kuku run [options] <prompt...>
```

Flags:

| Flag | Meaning |
|---|---|
| `-y`, `--yes` | 自动允许一次权限请求 |
| `--model <name>` | tier 名称或裸模型 ID |
| `-s`, `--session <id>` | 继续某个 Session |
| `-c`, `--continue` | 继续最近一次 Session |
| `--json` | 输出一行最终 JSON |
| `--stream-json` | 输出实时 JSON 行 |
| `--show-thinking` | 显示 thinking 内容 |
| `--raw` | 纯文本输出 |
| `--config <path>` | 使用指定配置文件 |
| `--prompts-dir <dir>` | 覆盖内嵌 Prompt 资源 |
| `--no-agents` | 禁用 `agent` Tool |
| `--no-skills` | 禁用 `use_skill` Tool |

如果 prompt 以 `/skill-name` 开头，且 skills 处于启用状态，`kuku run` 会加载这个 Skill，并把剩余文本作为用户 prompt 发送。

## `kuku show`

```text
kuku show <session-id>
```

## `kuku events`

```text
kuku events [-v|-vv] <session-id>
```

- `-v` 显示元数据
- `-vv` 显示完整上下文

## `kuku list`

```text
kuku list [--all] [--workspace <path>] [--verbose]
```

## `kuku delete`

```text
kuku delete [--workspace <path>] <session-id>
```

## `kuku config`

```text
kuku config [--config <path>] [show|validate|set|policy]
```

子命令：

| Subcommand | Syntax |
|---|---|
| show | `kuku config show` |
| validate | `kuku config validate` |
| set | `kuku config set <key> <value>` |
| policy allow | `kuku config policy allow <risk>` |
| policy deny | `kuku config policy deny <risk>` |

`policy allow` 和 `policy deny` 目前只会输出尚未实现的提示，而不会编辑 `policy.md`。

## `kuku prompts`

```text
kuku prompts [show [name] | export <dir>]
```

有效的 `show` 名称：

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
```

默认值：

- `--listen 127.0.0.1:17777`
- `--max-concurrent-runs 16`

请求和流格式见 [Server API](server-api.md)。
