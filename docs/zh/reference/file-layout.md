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
├── memory.md
├── packages/
├── agents/
├── skills/
├── logs/
└── p/
```

`logs/` 存放可观测性日志。Session 事实仍保存在各个 Session 的 `events.jsonl` 中。

当通过 `KUKU_PROVIDER_TRACE=1` 开启 provider trace 时，请求和响应诊断信息会写入：

```text
$KUKU_HOME/logs/provider-trace/<yyyy-mm-dd>/<session-id>.jsonl
```

## Project-Scoped Layout

对于工作区 `/code/my-app`，项目 home 是：

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

`events.jsonl` 是权威事实。操作系统锁保护唯一 writer，锁文件可在关闭后保留。`outputs/` 在 Session 生命周期内保留有界帧 stdout/stderr。公开 SDK 在日志中使用扁平 Agent 地址，不设每 Agent 目录或隐藏别名。旧 Session 的历史回滚备份可继续保留；新 Session API 不原地改写已接纳上下文。

所有权见 [Sessions](../how-it-works/sessions.md)，有界读取与保留边界见 [SDK](sdk.md)。

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
