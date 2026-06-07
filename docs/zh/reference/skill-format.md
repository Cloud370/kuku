# Skill Format

## 位置

- 用户级：`~/.kuku/skills/<name>/`
- 项目级：`<workspace>/.kuku/skills/<name>/`

## 必需布局

```text
<skill>/
├── SKILL.md
├── references/
├── scripts/
├── examples/
└── assets/
```

只有 `SKILL.md` 是必需的。

## `SKILL.md` 形状

`SKILL.md` 使用 YAML frontmatter，后面跟 Markdown 指令。

```markdown
---
name: tdd
description: Write tests before implementation. Follow red-green-refactor.
---

# TDD

Before writing any code:

1. Write a failing test
2. Write minimal code to pass
3. Refactor
```

## Frontmatter 字段

必填：

| Field | Rule |
|---|---|
| `name` | 1-64 个字符，小写字母、数字、连字符；必须与目录名匹配 |
| `description` | 1-1024 个字符；说明 Skill 做什么以及何时使用 |

可选：

| Field | Type | Meaning |
|---|---|---|
| `allowed-tools` | string[] | 仅为遗留元数据；不是运行时强制策略 |
| `disallowed-tools` | string[] | 仅为遗留元数据；不是运行时强制策略 |
| `max-turns` | integer | 激活时的轮次上限 |
| `model` | string | tier 覆盖 |
| `metadata` | table | 任意元数据 |

Skill 会指导模型行为，但不会改变权限。Tool 访问和权限提示仍然由当前 Session 的运行时策略决定。

## 运行时快照

每个 turn 开始时，运行时会发现当前可用的 Skill 定义，并把这个目录快照写入 Session 事件日志。只有在 `plugin.enabled` 开启时，package 提供的 Skill 才会参与这个快照。

- `list_skills` 和 `search_skills` 读取当前 turn 的快照。
- `use_skill` 也从同一个快照加载完整指令，而不是在本 turn 中稍后再次从磁盘重新读取。
- 如果一个 turn 被恢复，运行时会恢复该 turn 已持久化的快照，而不是重新发现磁盘上的最新 Skill。

磁盘上的变更会在下一次新的 turn 中生效，不会在 turn 中途生效，也不会在恢复旧 turn 时生效。

运行时不会单独预加载 `references/`、`scripts/`、`examples/` 和 `assets/`。这些目录仍然保留在 Skill 目录下，供指令或工具按需引用。

## Path Resolution

当 Skill 被加载时，运行时会在前面加入类似下面的注释：

```markdown
<!-- loaded: /home/user/.kuku/skills/tdd -->
```

Skill 指令中使用的相对路径会相对于这个 Skill 目录解析。
