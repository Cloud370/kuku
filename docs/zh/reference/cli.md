# CLI

## 入口模式

- `kuku` 在当前工作目录打开前台逐行输入界面，两轮之间可输入 `exit` 或 `quit` 退出。
- `kuku run ...` 执行非交互任务。

## 顶层命令

| 命令 | 用途 |
|---|---|
| `kuku run <prompt...>` | 通过公开 Session SDK 执行任务 |
| `kuku reply <session-id> <request-id>` | 回复持久问题或授权请求 |
| `kuku show <session-id>` | 查看 Session 的最终输出 |
| `kuku events <session-id>` | 查看持久 Session 事实 |
| `kuku list` | 列出 Session |
| `kuku delete <session-id>` | 删除 Session |
| `kuku config ...` | 查看、验证或修改配置 |
| `kuku init` | 初始化配置和运行目录 |
| `kuku prompts ...` | 查看或导出 Prompt 资产 |
| `kuku agents ...` | 列出或查看 Agent 联系卡 |
| `kuku skills ...` | 列出或查看 Skill |
| `kuku server ...` | 启动 HTTP API 服务 |
| `kuku web ...` | 启动包含内嵌 Web UI 的 HTTP 服务 |

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
| `--workspace <path>` | 工作目录，默认当前目录 |
| `--data-dir <path>` | 持久数据目录，默认 `$KUKU_HOME` 或 `~/.kuku` |
| `--command-id <id>` | 调用方指定的输入幂等标识 |
| `--reasoning-effort <value>` | Provider 支持的思考设置 |
| `--service-tier <value>` | Provider 支持的服务设置 |
| `--max-output-tokens <n>` | 单次模型响应的 token 上限 |
| `-s`, `--session <id>` | 继续指定 Session |
| `-c`, `--continue` | 继续最近的 Session |
| `--json` | 输出一行最终 JSON |
| `--stream-json` | 实时输出 JSON 行 |
| `--show-thinking` | 显示思考内容 |
| `--raw` | 纯文本输出 |
| `--verbose` | 显示持久 Agent 快照、配置和用量 |
| `--config <path>` | 指定配置文件 |
| `--prompts-dir <dir>` | 覆盖内嵌 Prompt 资产 |
| `--no-agents` | 禁用 `create_agent`、`send_message` 和 `list_agents`，保留 `ask_user` |
| `--no-skills` | 禁用默认 Skill 工具 `list_skills`、`search_skills`、`use_skill` 及斜杠 Skill 加载 |

本地 CLI 使用 SDK 的 trusted-local 策略，无需 `--yes` 即可运行文件和命令工具。Session 持有执行；Ctrl-C 会取消 Agent 并等待清理，每次运行都会等待异步 Session close。本地 Harness 正常关闭前会等待已在运行的 Agent 结算。打开 Session 或重放命令不会恢复已停止/中断的工作；新的 `run` 命令会显式推进目标 Agent。等待中的 Agent 需要匹配的回复。Turn 完成代表执行结束，任务是否成功仍需检查文件或命令判定结果。

Tier 名称使用 `--tier`。`--model` 选择精确模型标识，`--connection` 即使在多个连接共用协议时也只选择指定名称。显式 `--model` 覆盖 Tier 的模型，重开 Agent 时同时提供这两个参数也遵循此优先级。显式设置只影响所选 Agent。重开时不传选择参数，会保留该 Agent 的配置与独立上下文。切换模型路由会清除该 Agent 先前的 reasoning/service 覆盖值，再应用新的显式设置。

使用 `--command-id` 重试同一输入时，返回原始持久回执和当前 Agent 状态。重试不会修改配置、重复工作、批准待决请求或授权恢复中断执行；冲突输入会失败。此路径下 `--json`/`--stream-json` 返回 `event: "acknowledged"`、`replayed: true`、`receipt` 和 `state`。新工作或配置应使用新的命令标识。

`--json` 输出最终 `type: "session"` 对象，含 `event`、`session_id`、`agent`、`status`、`response`、`config`、`usage`、`permission` 和 `question`。`--stream-json` 另含瞬时进度、持久回执信息及最终持久结果。慢观察者可能丢失瞬时增量；最终响应来自已提交状态。Provider 用量及未知值遵循 [Session SDK](../how-it-works/sessions.md) 约定。

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

不启动执行运行时，读取最后的持久响应。`--json` 返回完整 Agent 快照，包括待决问题/授权标识。`show` 和指定 Session 的 `list` 使用[离线账本快照](sdk.md#操作)，不查询活动运行时；仅凭其状态无法判定活动执行是否已经停止。

## `kuku events`

```text
kuku events [-v|-vv] [--agent <address>] [--after <id>] [--limit <n>] [--json] <session-id>
```

- `-v` 或 `-vv` 增加简要显示的细节。
- `--json` 输出完整的持久事件记录。
- `--after` 为排他的持久游标；`--limit` 默认 100，接受 1–256 条。地址过滤在此有界账本页内进行。即使所有记录被过滤，最后扫描的游标也会输出到 stderr。
- `--workspace`、`--data-dir` 指定 Session 存储位置。
- 检查命令仍接受 `--conversation` 作为 `--agent` 别名。

命令读取 Session 的 `events.jsonl` 事实日志。可观察性日志位于 `$KUKU_HOME/logs/`，详见 [Session](file-layout.md)。

## `kuku list`

```text
kuku list [<session-id>] [--all] [--workspace <path>] [--data-dir <path>] [--verbose]
```

指定 Session 后列出稳定 Agent 地址、状态和待接纳输入数量。

## `kuku delete`

```text
kuku delete [--workspace <path>] <session-id>
```

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
```

默认值：

- `--listen 127.0.0.1:17777`
- `--max-concurrent-runs 16`

默认构建即可运行 `kuku server`。要运行 `kuku web`，先构建浏览器资源，再将它们嵌入统一二进制。安装 Node.js 24 和 npm 后，在仓库根目录运行以下命令，再在浏览器打开 `http://127.0.0.1:17777`。

```bash
npm --prefix apps/web ci
npm --prefix apps/web run build
cargo run --locked -p kuku-app --features embedded-web-assets -- web
```

请求及流格式见 [Server API](server-api.md)。
