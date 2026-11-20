# File Layout

## kuku Home

默认运行时 home：

```text
~/.kuku/
```

或者，如果设置了：

```text
$KUKU_HOME/
```

## Top-Level Layout

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

`logs/` 存放可观测性日志。Session 事实仍保存在各个 Session 的 `events.jsonl` 中。

`credentials/` 保存交互式设置创建的私有、不可变明文密钥文件。配置通过 `api_key = "file:<绝对路径>"` 引用它们；密钥不会复制到 TOML，也不会由 `kuku config show` 显示。凭据目录必须位于工作区之外。Unix 下目录权限为 `0700`，每个密钥文件为 `0600`（仅所有者可访问）；Windows 下要求受保护的 ACL，仅允许当前用户访问。遇到符号链接或权限不安全的路径时会拒绝使用，不会自动修复。文件未加密，配置更改后不会自动删除。

当通过 `KUKU_PROVIDER_TRACE=1` 开启 provider trace 时，请求和响应诊断信息会写入：

```text
$KUKU_HOME/logs/provider-trace/<yyyy-mm-dd>/<session-id>.jsonl
```

## Project-Scoped Layout

kuku 按已有工作目录的实际路径定位 Session、项目 Memory 和权限策略。通过符号链接打开同一目录时，可以读取同一份数据。工作区 `/code/my-app` 对应下面的项目数据目录。

```text
$KUKU_HOME/p/code/my-app/
```

该目录可能包含：

```text
$KUKU_HOME/p/<workspace>/
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

`events.jsonl` 是权威事实。操作系统锁保护唯一 writer，锁文件可在关闭后保留。`outputs/` 按有界帧保存命令捕获、派生输出和命名值的内容。日志单独记录命名和分享。公开 SDK 在日志中使用扁平 Agent 地址，不设每 Agent 目录或隐藏别名。旧 Session 的历史回滚备份可继续保留。Session API 保留已接纳的上下文。

所有权见 [Sessions](../how-it-works/sessions.md)，有界读取与保留边界见 [SDK](sdk.md)。

## Directory Creation Intents

在 Unix 上，SDK 创建目录时可能在其直接父目录内临时放置一个空的、仅所有者可读写的 `.kuku-mkdir-v1-<sha256>.intent` 文件。摘要是子目录原始文件名字节的 SHA-256。文件锁串行协调该子目录的创建与恢复；它不是 PID 租约。

SDK 在 `mkdir` 前同步意图文件及其父目录，随后同步新目录和父目录，最后删除意图并再次同步父目录。后续创建或重开会沿请求路径检查精确的意图文件名，先完成中断的事务再向下推进；首次创建嵌套数据目录时中断也由此恢复。没有意图的既有目录无需同步，因此未改动的祖先目录只需遍历权限。若等待者锁住的意图已被移除，必须重新打开并锁定当前 inode，才能修改目录。

发布失败会保留意图供恢复。SDK 资源可能正在打开时，不要移除这些文件；创建成功后会清理它们。Windows 文件发布使用写穿文件句柄、完整文件刷新和写穿移动，不采用 Unix 意图协议。契约测试覆盖进程退出后重开、并发创建、意图 inode 替换及目录权限；它们不证明某个文件系统或设备的断电行为。

## User and Project Extensions

约定的用户级定义位置：

- `~/.kuku/agents/`
- `~/.kuku/skills/`
- `~/.kuku/packages/`

工作区内约定的项目级定义位置：

- `<workspace>/.kuku/agents/`
- `<workspace>/.kuku/skills/`
- `<workspace>/.kuku/packages/`

当 `auto_discover = true` 时，kuku 也会扫描其他用户和项目 dot-directory 下的 `skills/`、`agents/` 和 `agent/` 子目录。像 `.claude/skills`、`.claude/agents`、`.opencode/agent` 这样的兼容布局也会被自动发现。
