# 文件布局

## kuku 数据目录

默认位于 `~/.kuku/`，设置 `KUKU_HOME` 后位于 `$KUKU_HOME/`：

```text
$KUKU_HOME/
├── config.toml
├── credentials/
├── memory.md
├── packages/
├── agents/
├── skills/
├── logs/
└── p/
```

- **`logs/`** — 可观测性日志，以及 `logs/provider-trace/` 下的轮转 [Provider Trace](environment-variables.md#provider-trace-日志)。Session 事实只保存在各 Session 的 `events.jsonl` 中。
- **`credentials/`** — 交互式配置写入的仅所有者可读明文密钥文件，配置中以 `api_key = "file:<绝对路径>"` 引用，密钥不会写进 `config.toml`，也不会被 `kuku config show` 打印。共用同一数据目录的工作区共享此目录（同一系统用户下的进程可读取）。Unix 下目录权限为 `0700`、文件为 `0600`；Windows 下要求受保护 ACL 仅允许当前用户访问。遇到符号链接或权限不安全的路径会直接拒绝，不自动修复。文件未加密，修改配置时也不会自动删除旧文件。

订阅账号独立保存到 `credentials/subscriptions/<渠道>/<账号>.json`。上级私有文件 `host.json` 保存稳定的主机 ID，以及从已授权邮箱派生不透明账号标识的本地密钥。每份账号记录保存脱敏邮箱、注册信息和当前令牌。凭据以仅所有者可访问的明文保存，遵循与已保存密钥相同的平台权限规则。续期和退出在跨进程锁内原子发布整份记录。退出清除令牌并保留注册信息。

## 项目级目录

kuku 按工作区解析后的规范路径定位 Session、项目 Memory 和权限策略（经由符号链接进入同一目录会命中同一份数据）。以 `/code/my-app` 为例：

```text
$KUKU_HOME/p/code/my-app/
├── memory.md
├── policy.md
└── sessions/
```

## Session Layout

```text
<data-dir>/p/<workspace>/sessions/<session-id>/
├── events.jsonl
├── events.jsonl.lock
└── outputs/<output-id>.output
```

- `events.jsonl` 是唯一权威账本；`events.jsonl.lock` 用于单写者操作系统锁（关闭后锁文件可留在磁盘）。
- `outputs/` 存放有界帧格式的命令输出、派生输出与命名值字节，命名和分享元数据记在日志里。
- Agent 在日志内使用扁平地址，没有按 Agent 拆分的子目录或隐藏别名。
- 日志默认以纯 JSONL 写入。[满足条件的正常关闭](../how-it-works/sessions.md#日志何时压缩)会把较早历史封存为带内嵌检查点的独立 gzip 帧，新事件继续以 JSONL 追加到同一文件尾部。`kuku events --file <path> --all --json` 对两种布局透明读取。[Session 事件](events.md) · [Session 机制](../how-it-works/sessions.md) · [SDK](sdk.md)

## 目录创建意图文件（Unix）

在 Unix 上创建目录时，SDK 可能在父目录临时放置一个空的仅所有者可读写锁文件 `.kuku-mkdir-v1-<sha256>.intent`（摘要取自子目录原始文件名字节），用于串行化目录创建与崩溃恢复。SDK 在 `mkdir` 前同步意图文件与父目录，创建后同步新目录与父目录，完成后删除意图文件。

SDK 资源可能正在打开时不要手动删除 `.intent` 文件——后续打开会自动完成被中断的创建。Windows 使用写穿句柄、完整刷盘和原子移动，不使用意图文件。

## 用户与项目扩展路径

约定目录：

- 用户级：`~/.kuku/agents/`、`~/.kuku/skills/`、`~/.kuku/packages/`
- 项目级：`<workspace>/.kuku/agents/`、`<workspace>/.kuku/skills/`、`<workspace>/.kuku/packages/`

开启 `auto_discover = true` 后，kuku 还会扫描其他用户级和项目级点目录下的 `skills/`、`agents/` 和 `agent/`（如 `.claude/skills`、`.claude/agents`、`.opencode/agent`）。
