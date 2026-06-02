# File Layout

## kuku Home

默认运行时 home：

```text
~/.kuku/
```

或者，如果已设置：

```text
$KUKU_HOME/
```

## 顶层布局

```text
$KUKU_HOME/
├── config.toml
├── memory.md
├── packages/
├── agents/
├── skills/
└── p/
```

## 项目作用域布局

对于 workspace `/code/my-app`，项目 home 是：

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

## Session 布局

```text
$KUKU_HOME/p/<workspace>/sessions/<session-id>/
├── lock
├── events.jsonl
├── pre-revert-<event-id>/
└── subs/
```

说明：

- `lock` 记录当前活跃写入者。
- `events.jsonl` 是规范的 Session 日志。
- `pre-revert-<event-id>/` 会在文件回滚创建备份时出现。
- `subs/` 用于存在子 Session 的情况。

## 用户级和项目级扩展

约定的用户级定义：

- `~/.kuku/agents/`
- `~/.kuku/skills/`
- `~/.kuku/packages/`

workspace 内约定的项目级定义：

- `<workspace>/.kuku/agents/`
- `<workspace>/.kuku/skills/`
- `<workspace>/.kuku/packages/`

当 `auto_discover = true` 时，kuku 也会扫描其他用户级和项目级 dot-directory 中的 `skills/`、`agents/` 和 `agent/` 子目录。像 `.claude/skills`、`.claude/agents`、`.opencode/agent` 这样的兼容布局会被自动发现。
