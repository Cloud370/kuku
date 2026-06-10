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
$KUKU_HOME/p/<workspace>/sessions/<session-id>/
├── lock
├── events.jsonl
└── pre-revert-<event-id>/
```

说明：

- `lock` 记录当前活跃写入者。
- `events.jsonl` 是 Session 账本。
- `pre-revert-<event-id>/` 会在文件回滚创建备份时出现。

`subs/` 已不再是 agent 工作的规范主模型。规范的 agent conversation 存在于同一个 Session 账本中，并通过 `events.jsonl` 里的 conversation address 来区分。

更老的 Session 里仍可能保留早期委派布局留下的兼容产物。应把这些视为历史遗留。

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
